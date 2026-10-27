//! 将剪贴板文件与图片准备为 pane 可读路径，并持有远端暂存目录与上传生命周期。
//! 不依赖终端视图或 grid；调用方只负责投递路径和呈现失败通知。

use std::path::{Path, PathBuf};

use gpui_kit::{AppContext as _, AsyncApp, ClipboardEntry, ClipboardItem, Task};

use crate::ui::sftp::SftpRoute;

/// 每个 pane 的远端粘贴状态；只缓存已经验证为私有目录的成功准备结果。
#[derive(Default)]
pub(super) struct RemotePaste {
    directory: Option<String>,
}

impl RemotePaste {
    /// 阻塞的目录准备与首次过期清理始终在后台执行。
    pub(super) fn prepare(
        &self,
        route: &SftpRoute,
        cx: &mut AsyncApp,
    ) -> Task<Result<String, String>> {
        if let Some(dir) = &self.directory {
            return Task::ready(Ok(dir.clone()));
        }
        let route = route.clone();
        cx.background_spawn(async move {
            let prepared = prepare_remote_clipboard_dir(&route);
            if let Ok(dir) = &prepared {
                prune_remote_clipboard_dir(&route, dir);
            }
            prepared
        })
    }

    /// 在持有者仍存活时提交准备结果；失败不会阻止下一次重试。
    pub(super) fn prepared(&mut self, prepared: Result<String, String>) -> Result<String, String> {
        self.directory = prepared.as_ref().ok().cloned();
        prepared
    }
}

/// 已尝试启动的一次粘贴；路径可立即投递，不等待传输完成。
pub(super) struct PasteUploads {
    uploads: Vec<PasteUpload>,
}

impl PasteUploads {
    pub(super) async fn start(
        route: SftpRoute,
        pane_id: u64,
        dir: String,
        sources: Vec<PathBuf>,
        cx: &mut AsyncApp,
    ) -> Result<Self, String> {
        cx.background_spawn(async move {
            start_paste_uploads(&route, pane_id, &dir, &sources).map(|uploads| Self { uploads })
        })
        .await
    }

    /// 未能启动的文件保留本地路径，避免部分失败吞掉用户的输入。
    pub(super) fn paths(&self) -> Vec<String> {
        self.uploads
            .iter()
            .map(|upload| match &upload.started {
                Ok(started) => started.remote.clone(),
                Err(_) => upload.local.to_string_lossy().into_owned(),
            })
            .collect()
    }

    pub(super) fn failures(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.uploads.iter().filter_map(|upload| {
            upload
                .started
                .as_ref()
                .err()
                .map(|reason| (upload.local.as_path(), reason.as_str()))
        })
    }

    pub(super) fn into_started(self) -> impl Iterator<Item = (PathBuf, StartedUpload)> {
        self.uploads
            .into_iter()
            .filter_map(|upload| upload.started.ok().map(|started| (upload.local, started)))
    }
}

impl StartedUpload {
    /// 即使 SFTP 面板关闭也观察完成；只限制无进展时长，不限制文件总传输时间。
    pub(super) async fn watch(&self, route: SftpRoute, cx: &mut AsyncApp) -> Result<(), String> {
        use crate::daemon::protocol::{SftpJobState, SftpOp};
        const POLL: std::time::Duration = std::time::Duration::from_millis(500);
        const STALLED_POLLS: usize = 600;
        let mut last_done = 0;
        let mut still = 0;
        while still < STALLED_POLLS {
            cx.background_executor().timer(POLL).await;
            let listed = {
                let route = route.clone();
                cx.background_spawn(async move { route.transfer_list() })
                    .await
            };
            // 查询失败不代表传输失败，仍按无进展时长继续观察。
            let Ok(listed) = listed else {
                still += 1;
                continue;
            };
            let Some(progress) = listed.into_iter().find(|job| job.job_id == self.job) else {
                // daemon 重启或已超出完成记录保留时间时，不再有可报告的结果。
                return Ok(());
            };
            match progress.state {
                SftpJobState::Running => {
                    still = if progress.bytes_done == last_done {
                        still + 1
                    } else {
                        0
                    };
                    last_done = progress.bytes_done;
                }
                SftpJobState::Done => {
                    // 文件再收紧为 0600；目录内容仍由已验证的 0700 暂存目录保护。
                    if !self.is_dir {
                        let (route, path) = (route.clone(), self.remote.clone());
                        cx.background_spawn(async move {
                            route.op(SftpOp::Chmod { path, mode: 0o600 })
                        })
                        .await;
                    }
                    return Ok(());
                }
                SftpJobState::Cancelled => return Ok(()),
                SftpJobState::Error => {
                    return Err(progress.error.unwrap_or_else(|| "upload failed".into()));
                }
            }
        }
        Ok(())
    }
}
/// The local files the clipboard carries — a Finder or Explorer Copy on a file
/// puts its path there, not its bytes.
pub(super) fn clipboard_paths(item: &ClipboardItem) -> Vec<std::path::PathBuf> {
    item.entries()
        .iter()
        .filter_map(|e| match e {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}
pub(super) fn write_clipboard_image(img: &gpui_kit::Image) -> Option<std::path::PathBuf> {
    use gpui_kit::ImageFormat;
    let dir = std::env::temp_dir().join("lyrel-clipboard");
    std::fs::create_dir_all(&dir).ok()?;
    let (ext, transcoded) = match img.format {
        ImageFormat::Png => ("png", None),
        ImageFormat::Jpeg => ("jpg", None),
        ImageFormat::Gif => ("gif", None),
        ImageFormat::Webp => ("webp", None),
        other => ("png", Some(transcode_to_png(&img.bytes, other)?)),
    };
    let data: &[u8] = transcoded.as_deref().unwrap_or(&img.bytes);
    let path = dir.join(format!("paste-{:016x}.{ext}", img.id));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// The connection a paste should be uploaded over, when this pane runs on a
/// remote host reachable over the daemon's russh stack: either a native SSH
/// workspace pane or a standalone native SSH pane. WSL shares localhost and
/// needs path translation instead, and a workspace without connection details
/// has no channel to piggyback on — both keep the local-path behavior.
pub(super) fn remote_paste_spec<'a>(
    workspace: Option<&'a crate::terminal::PaneWorkspace>,
    ssh_spec: Option<&'a crate::daemon::protocol::NativeSshSpec>,
) -> Option<&'a crate::daemon::protocol::NativeSshSpec> {
    if let Some(ws) = workspace {
        if ws.shares_localhost() {
            return None;
        }
        return ws.spec.as_deref();
    }
    ssh_spec
}
/// Whether a pane stages the clipboard image to a file instead of forwarding
/// SYN and letting the agent read the clipboard itself.
///
/// SYN only works because the agent shares a clipboard with the pane. That
/// holds for a local macOS pane — where it is the better path, carrying the
/// image at full fidelity — and nowhere else: off macOS Claude Code silently
/// drops raw screenshots (anthropics/claude-code#26679), and an agent in a
/// remote pane reads the clipboard of the host it runs on, which never holds
/// this machine's screenshot no matter what the local OS is.
pub(super) fn stages_clipboard_image(is_remote: bool) -> bool {
    cfg!(not(target_os = "macos")) || is_remote
}

/// The WSL view of a Windows path: `C:\x\y` becomes `/mnt/c/x/y`.
///
/// `None` for anything without a drive letter — a UNC temp directory has no
/// automount mapping, and `C:x` is drive-relative rather than absolute.
///
/// The `/mnt` prefix is WSL's default automount root, not a guaranteed one:
/// `[automount] root=` in `/etc/wsl.conf` can move it. Asking the distro
/// (`wslpath -u`) would be exact, but it is a round trip through the daemon on
/// a keystroke path, and a moved automount root is rare enough that a wrong
/// path — which the user sees, in their own line, before they send it — beats
/// making every paste wait on a subprocess.
fn wsl_path(windows: &str) -> Option<String> {
    let mut chars = windows.chars();
    let drive = chars.next()?.to_ascii_lowercase();
    if !drive.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    if !matches!(chars.next()?, '\\' | '/') {
        return None;
    }
    Some(format!(
        "/mnt/{drive}/{}",
        chars.as_str().replace('\\', "/")
    ))
}
/// A local path — a staged image, a copied or dropped file — as the pane's own
/// filesystem spells it.
///
/// A WSL pane shares this machine's disk but not its path syntax: an agent in
/// there reads `/mnt/c/…` and cannot open `C:\…` at all, which is why the
/// upload route skips WSL — there is nothing to copy, only a name to rewrite.
/// A path with no mapping falls back to the Windows one, which at least tells
/// the user where the file is.
pub(super) fn local_path_for_pane(local: &str, shares_localhost: bool) -> String {
    if shares_localhost {
        return wsl_path(local).unwrap_or_else(|| local.to_string());
    }
    local.to_string()
}

/// Staging pastes under the SSH user's own home keeps them out of the
/// world-writable `/tmp`, where any local account could pre-create the
/// directory, read what lands in it, or swap a pasted file for one of its own
/// before the pane's agent opens it.
const REMOTE_CLIPBOARD_PATH: [&str; 3] = [".cache", "lyrel", "clipboard"];

/// Owner-only, and *only* owner: a staging directory anyone else can enter is
/// one anyone else can read the pasted files out of.
const REMOTE_CLIPBOARD_MODE: u32 = 0o700;

/// How long a paste stays staged on the remote. Long enough to outlive any
/// conversation that still refers to it; a week of pasted spreadsheets is
/// what would otherwise pile up in the user's home forever.
const REMOTE_CLIPBOARD_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// A fresh name for one paste's own directory under the staging directory.
///
/// Each paste gets a directory so every file keeps its real name — which is
/// what the agent reads, and `report.xlsx` says far more than a hash — without
/// two pastes of the same name overwriting each other.
fn paste_dir_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("paste-{nanos:x}{n:x}")
}

/// Whether a staging entry has outlived [`REMOTE_CLIPBOARD_TTL_SECS`]. An
/// entry with no mtime (`0`) is left alone: its age is unknown, not great.
fn staged_entry_expired(mtime: u64, now: u64) -> bool {
    mtime != 0 && now.saturating_sub(mtime) > REMOTE_CLIPBOARD_TTL_SECS
}

/// Remove the staging directory's expired entries. Best effort: a failure
/// here costs disk space, never a paste, so nothing is reported.
fn prune_remote_clipboard_dir(route: &crate::ui::sftp::SftpRoute, dir: &str) {
    let Ok(entries) = route.list(dir) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for entry in entries {
        if entry.name == "." || entry.name == ".." || !staged_entry_expired(entry.mtime, now) {
            continue;
        }
        remove_remote_tree(
            route,
            &crate::daemon::ssh::sftp::remote_join(dir, &entry.name),
            entry.kind,
            0,
        );
    }
}

/// Delete a remote file or directory tree. A symlink is removed as a link and
/// never followed — the tree is ours, where it points is not.
fn remove_remote_tree(
    route: &crate::ui::sftp::SftpRoute,
    path: &str,
    kind: crate::daemon::protocol::SftpEntryKind,
    depth: usize,
) {
    use crate::daemon::protocol::{SftpEntryKind, SftpOp};
    // A pasted folder is as deep as it was on disk; this bounds a pathological
    // one rather than any real one.
    const MAX_DEPTH: usize = 64;
    if kind != SftpEntryKind::Dir {
        route.op(SftpOp::RemoveFile {
            path: path.to_string(),
        });
        return;
    }
    if depth < MAX_DEPTH
        && let Ok(children) = route.list(path)
    {
        for child in children {
            if child.name == "." || child.name == ".." {
                continue;
            }
            let child_path = crate::daemon::ssh::sftp::remote_join(path, &child.name);
            remove_remote_tree(route, &child_path, child.kind, depth + 1);
        }
    }
    route.op(SftpOp::RemoveDir {
        path: path.to_string(),
    });
}

/// One local path handed to the remote: where it went and the transfer
/// carrying it, or why it never left.
struct PasteUpload {
    local: std::path::PathBuf,
    started: Result<StartedUpload, String>,
}

pub(super) struct StartedUpload {
    remote: String,
    job: u64,
    is_dir: bool,
}

/// Start uploading `sources` into a new directory of their own under the
/// staging directory `dir`. Blocking, like everything that talks to the
/// daemon, so this runs off the UI thread. `Err` means the paste's directory
/// could not be made and nothing was started; a source that could not be
/// started on its own is reported in its own entry.
fn start_paste_uploads(
    route: &crate::ui::sftp::SftpRoute,
    pane_id: u64,
    dir: &str,
    sources: &[std::path::PathBuf],
) -> Result<Vec<PasteUpload>, String> {
    use crate::daemon::protocol::{SftpOp, SftpOpResult, SftpTransferKind, SftpTransferSpec};
    let paste_dir = crate::daemon::ssh::sftp::remote_join(dir, &paste_dir_name());
    if let SftpOpResult::Error(e) = route.op(SftpOp::Mkdir {
        path: paste_dir.clone(),
    }) {
        return Err(e);
    }
    Ok(sources
        .iter()
        .map(|local| {
            let started = (|| {
                let name = local
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| crate::daemon::ssh::sftp::safe_local_name(n))
                    .ok_or_else(|| format!("{} has no file name", local.display()))?;
                let is_dir = std::fs::metadata(local)
                    .map_err(|e| format!("{}: {e}", local.display()))?
                    .is_dir();
                let remote = crate::daemon::ssh::sftp::remote_join(&paste_dir, &name);
                let job = route.transfer_start(SftpTransferSpec {
                    pane_id,
                    kind: SftpTransferKind::Upload,
                    local: local.clone(),
                    remote: remote.clone(),
                    recursive: is_dir,
                })?;
                Ok(StartedUpload {
                    remote,
                    job,
                    is_dir,
                })
            })();
            PasteUpload {
                local: local.clone(),
                started,
            }
        })
        .collect())
}

/// Whether a prepared staging directory may be uploaded into.
///
/// The mode is what a `stat` reported *after* a `chmod 0700` the daemon
/// watched succeed, which is the ownership proof: POSIX only lets a file's
/// owner change its mode, so a directory lyrel can chmod and then observe at
/// exactly `0700` is one the SSH user owns and nobody else can enter. A
/// symlink is refused outright because `stat` follows links, so a link planted
/// at the staging path would otherwise be judged by its target.
fn staging_dir_is_safe(
    is_symlink: bool,
    kind: Option<crate::daemon::protocol::SftpEntryKind>,
    mode: u32,
) -> bool {
    use crate::daemon::protocol::SftpEntryKind;
    !is_symlink
        && matches!(kind, Some(SftpEntryKind::Dir))
        && mode & 0o7777 == REMOTE_CLIPBOARD_MODE
}

/// Create and verify the per-user staging directory, answering the absolute
/// remote path to upload into. Blocking: every step is a daemon round trip
/// over the pane's SSH connection, so this only ever runs off the UI thread.
fn prepare_remote_clipboard_dir(route: &crate::ui::sftp::SftpRoute) -> Result<String, String> {
    use crate::daemon::protocol::{SftpOp, SftpOpResult};
    let home = match route.op(SftpOp::Realpath {
        path: ".".to_string(),
    }) {
        SftpOpResult::Link(home) if home.starts_with('/') => home,
        SftpOpResult::Error(e) => return Err(e),
        other => {
            return Err(format!(
                "the remote home directory is not a path: {other:?}"
            ));
        }
    };
    let mut dir = home;
    for component in REMOTE_CLIPBOARD_PATH {
        dir = crate::daemon::ssh::sftp::remote_join(&dir, component);
        // An existing directory fails here with EEXIST; the checks below are
        // what decide whether this one is ours, so the result carries no
        // information worth branching on.
        let _ = route.op(SftpOp::Mkdir { path: dir.clone() });
    }
    if let SftpOpResult::Link(target) = route.op(SftpOp::Readlink { path: dir.clone() }) {
        return Err(format!("{dir} is a symlink to {target}"));
    }
    if let SftpOpResult::Error(e) = route.op(SftpOp::Chmod {
        path: dir.clone(),
        mode: REMOTE_CLIPBOARD_MODE,
    }) {
        return Err(format!("{dir} is not owned by this session: {e}"));
    }
    match route.op(SftpOp::Stat { path: dir.clone() }) {
        SftpOpResult::Stat(entry)
            if staging_dir_is_safe(false, Some(entry.kind), entry.permissions) =>
        {
            Ok(dir)
        }
        SftpOpResult::Stat(entry) => Err(format!(
            "{dir} is not a private directory (mode {:o})",
            entry.permissions & 0o7777
        )),
        SftpOpResult::Error(e) => Err(e),
        other => Err(format!("unexpected reply for {dir}: {other:?}")),
    }
}

fn transcode_to_png(bytes: &[u8], format: gpui_kit::ImageFormat) -> Option<Vec<u8>> {
    use gpui_kit::ImageFormat as G;
    let src = match format {
        G::Png => image::ImageFormat::Png,
        G::Jpeg => image::ImageFormat::Jpeg,
        G::Webp => image::ImageFormat::WebP,
        G::Gif => image::ImageFormat::Gif,
        G::Bmp => image::ImageFormat::Bmp,
        G::Tiff => image::ImageFormat::Tiff,
        G::Ico => image::ImageFormat::Ico,
        G::Pnm => image::ImageFormat::Pnm,
        G::Svg => return None,
    };
    let decoded = image::load_from_memory_with_format(bytes, src).ok()?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::{RemoteTarget, WorkspaceId};
    use crate::terminal::PaneWorkspace;
    use gpui_kit::ExternalPaths;

    fn ws(target: RemoteTarget, with_spec: bool) -> PaneWorkspace {
        PaneWorkspace {
            workspace: WorkspaceId::new(),
            target,
            spec: with_spec.then(|| Box::new(native_spec())),
            label: None,
            resize_echo: false,
        }
    }
    /// The SSH user a paste would be uploaded for, or `None` when the pane
    /// keeps the local-path behavior.
    fn remote_paste_user<'a>(
        workspace: Option<&'a crate::terminal::PaneWorkspace>,
        ssh_spec: Option<&'a crate::daemon::protocol::NativeSshSpec>,
    ) -> Option<&'a str> {
        remote_paste_spec(workspace, ssh_spec).map(|s| s.user.as_str())
    }

    fn native_spec() -> crate::daemon::protocol::NativeSshSpec {
        serde_json::from_str(r#"{"host":"dev.box","port":22,"user":"me","auth_mode":"auto"}"#)
            .unwrap()
    }

    #[test]
    fn local_pane_pastes_the_local_image_path() {
        assert_eq!(remote_paste_user(None, None), None);
    }

    #[test]
    fn ssh_workspace_panes_upload_images_for_the_ssh_user() {
        let w = ws(RemoteTarget::direct("me", "dev.box", 22), true);
        assert_eq!(remote_paste_user(Some(&w), None), Some("me"));
    }

    #[test]
    fn standalone_ssh_panes_upload_images_for_the_ssh_user() {
        let spec = native_spec();
        assert_eq!(remote_paste_user(None, Some(&spec)), Some("me"));
    }
    #[test]
    fn wsl_and_specless_workspaces_keep_the_local_image_path() {
        let wsl = ws(
            RemoteTarget::Wsl {
                distro: "Ubuntu".into(),
            },
            false,
        );
        assert_eq!(remote_paste_user(Some(&wsl), None), None);
        let bare = ws(RemoteTarget::direct("me", "dev.box", 22), false);
        assert_eq!(remote_paste_user(Some(&bare), None), None);
    }

    #[test]
    fn a_staging_dir_is_only_safe_when_it_is_a_private_directory_we_own() {
        use crate::daemon::protocol::SftpEntryKind;
        // `chmod 0700` succeeded and the mode came back as asked: ours.
        assert!(staging_dir_is_safe(
            false,
            Some(SftpEntryKind::Dir),
            0o040700
        ));
        // A mode anyone else can enter is one anyone else can read pastes from.
        assert!(!staging_dir_is_safe(
            false,
            Some(SftpEntryKind::Dir),
            0o040755
        ));
        assert!(!staging_dir_is_safe(
            false,
            Some(SftpEntryKind::Dir),
            0o040701
        ));
        // Sticky/setgid bits mean someone else set the terms.
        assert!(!staging_dir_is_safe(
            false,
            Some(SftpEntryKind::Dir),
            0o041700
        ));
        // A symlink is judged by its target by `stat`, so refuse it outright.
        assert!(!staging_dir_is_safe(
            true,
            Some(SftpEntryKind::Dir),
            0o040700
        ));
        // A file (or a path that vanished) is not a staging dir.
        assert!(!staging_dir_is_safe(
            false,
            Some(SftpEntryKind::File),
            0o100700
        ));
        assert!(!staging_dir_is_safe(false, None, 0o040700));
    }

    #[test]
    fn a_remote_pane_stages_its_clipboard_image_on_every_platform() {
        // The SYN path leans on the agent sharing a clipboard with the pane,
        // which a remote agent never does — it reads the clipboard of the host
        // it runs on, and this machine's screenshot is not in it.
        assert!(
            stages_clipboard_image(true),
            "a remote agent cannot see this machine's clipboard"
        );
        // Locally the platform decides: macOS hands the agent the clipboard
        // itself, which is higher fidelity than a staged file.
        assert_eq!(
            stages_clipboard_image(false),
            cfg!(not(target_os = "macos"))
        );
    }
    #[test]
    fn a_wsl_pane_gets_the_automount_path_not_the_windows_one() {
        // The staged file really is on the pane's own disk — only its name
        // differs — so this is a rewrite, not an upload.
        assert_eq!(
            local_path_for_pane(
                r"C:\Users\me\AppData\Local\Temp\lyrel-clipboard\paste-1.png",
                true
            ),
            "/mnt/c/Users/me/AppData/Local/Temp/lyrel-clipboard/paste-1.png"
        );
        assert_eq!(wsl_path(r"D:\x\y.png").as_deref(), Some("/mnt/d/x/y.png"));

        // No automount mapping: the Windows path at least says where it went.
        let unc = r"\\server\share\paste-1.png";
        assert_eq!(wsl_path(unc), None);
        assert_eq!(local_path_for_pane(unc, true), unc);
        // Drive-relative, not absolute — `C:x` means "x under C:'s cwd".
        assert_eq!(wsl_path(r"C:paste-1.png"), None);

        // Every other pane keeps the path exactly as staged.
        assert_eq!(
            local_path_for_pane("/tmp/lyrel-clipboard/paste-1.png", false),
            "/tmp/lyrel-clipboard/paste-1.png"
        );
        assert_eq!(
            local_path_for_pane(r"C:\Temp\paste-1.png", false),
            r"C:\Temp\paste-1.png"
        );
    }

    #[test]
    fn the_pasted_image_name_stands_alone_as_a_remote_path_component() {
        use crate::daemon::ssh::sftp::safe_local_name;
        use gpui_kit::{Image, ImageFormat};

        let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(pixel)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let path = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Png, png)).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(safe_local_name(&name), "{name} must not traverse or nest");
    }
    #[test]
    fn clipboard_image_transcodes_bmp_to_png_and_passes_png_through() {
        use gpui_kit::{Image, ImageFormat};

        let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
        let mut bmp = Vec::new();
        image::DynamicImage::ImageRgba8(pixel)
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        let path = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Bmp, bmp)).unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");

        let png = std::fs::read(&path).unwrap();
        let out = super::write_clipboard_image(&Image::from_bytes(ImageFormat::Png, png.clone()))
            .unwrap();
        assert_eq!(out.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&out).unwrap(), png);
    }

    /// A macOS screenshot reaches the pasteboard as TIFF, which agent vision
    /// rejects the same way it rejects a Windows BMP. The transcode depends on
    /// `image`'s TIFF decoder being compiled in — without it this fails at
    /// runtime by quietly staging nothing, which reads as "paste did nothing".
    #[test]
    fn a_macos_tiff_screenshot_is_staged_as_png() {
        use gpui_kit::{Image, ImageFormat};

        let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
        let mut tiff = Vec::new();
        image::DynamicImage::ImageRgba8(pixel)
            .write_to(
                &mut std::io::Cursor::new(&mut tiff),
                image::ImageFormat::Tiff,
            )
            .unwrap();
        let path =
            super::write_clipboard_image(&Image::from_bytes(ImageFormat::Tiff, tiff)).unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
    }
    #[test]
    fn copied_files_keep_their_paths_and_text_does_not_become_a_file() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![
                    PathBuf::from("/Users/me/My File.txt"),
                    PathBuf::from("/tmp/b.log"),
                ]
                .into(),
            ))],
        };
        let paths = clipboard_paths(&item);
        assert_eq!(
            paths,
            [
                PathBuf::from("/Users/me/My File.txt"),
                PathBuf::from("/tmp/b.log")
            ]
        );
        // Text is text: nothing on the clipboard names a file.
        let text = ClipboardItem::new_string("echo hi".to_string());
        assert!(clipboard_paths(&text).is_empty());
    }

    /// Two pastes of `report.xlsx` in the same instant must not land in the
    /// same directory, or the second overwrites the file the first pasted.
    #[test]
    fn every_paste_stages_into_a_directory_of_its_own() {
        let a = super::paste_dir_name();
        let b = super::paste_dir_name();
        assert_ne!(a, b);
        assert!(crate::daemon::ssh::sftp::safe_local_name(&a));
    }

    #[test]
    fn staged_pastes_expire_after_a_week_and_never_on_an_unknown_age() {
        let now = 1_800_000_000;
        let week = super::REMOTE_CLIPBOARD_TTL_SECS;
        assert!(!super::staged_entry_expired(now - 60, now));
        assert!(!super::staged_entry_expired(now - week, now));
        assert!(super::staged_entry_expired(now - week - 1, now));
        // No mtime from the server says nothing about age.
        assert!(!super::staged_entry_expired(0, now));
        // A remote clock ahead of ours is young, not negative.
        assert!(!super::staged_entry_expired(now + 3600, now));
    }

    #[test]
    fn partially_started_uploads_paste_remote_and_local_paths_in_source_order() {
        let uploads = PasteUploads {
            uploads: vec![
                PasteUpload {
                    local: PathBuf::from("/local/report.xlsx"),
                    started: Ok(StartedUpload {
                        remote: "/home/me/.cache/lyrel/clipboard/paste-a/report.xlsx".into(),
                        job: 42,
                        is_dir: false,
                    }),
                },
                PasteUpload {
                    local: PathBuf::from("/local/missing.txt"),
                    started: Err("file vanished".into()),
                },
            ],
        };
        assert_eq!(
            uploads.paths(),
            [
                "/home/me/.cache/lyrel/clipboard/paste-a/report.xlsx",
                "/local/missing.txt",
            ]
        );
        assert_eq!(
            uploads.failures().collect::<Vec<_>>(),
            [(Path::new("/local/missing.txt"), "file vanished")]
        );
        let started = uploads.into_started().collect::<Vec<_>>();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].0, Path::new("/local/report.xlsx"));
        assert_eq!(started[0].1.job, 42);
    }

    #[test]
    fn a_batch_with_no_started_upload_still_pastes_each_local_path() {
        let uploads = PasteUploads {
            uploads: vec![PasteUpload {
                local: PathBuf::from("/local/missing.txt"),
                started: Err("permission denied".into()),
            }],
        };
        assert_eq!(uploads.paths(), ["/local/missing.txt"]);
        assert_eq!(uploads.failures().count(), 1);
        assert_eq!(uploads.into_started().count(), 0);
    }
}
