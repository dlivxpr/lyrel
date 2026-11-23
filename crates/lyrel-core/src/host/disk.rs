//! The bounded directory scan and volume lookup behind the disk wire values.
//!
//! Injected into the full local Host at construction; every control
//! connection that asks for a directory scan walks through this one
//! implementation. The walk uses real directory enumeration, never follows
//! symbolic links or junctions, stops at an explicit entry bound and says
//! when it did. The volume lookup addresses the canonical scan root directly.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use lyrel_protocol::host::disk::*;

/// The walk visits at most this many directory entries before stopping and
/// reporting a partial total. This is the scan's real work ceiling: a huge
/// tree returns in bounded time with a floor total, instead of outliving the
/// RPC deadline that cannot cancel it anyway. One budget counts every entry
/// enumeration, so the ceiling holds on trees deep in either dimension.
const MAX_ENTRIES: u64 = 200_000;

/// At most this many representative entry errors ride along in a partial
/// scan, so one unreadable subtree cannot balloon the reply.
const MAX_ERRORS: usize = 8;

/// One bounded scan of `dir` on this machine, stamped with this daemon's
/// instance so a late answer from a replaced peer is rejected by its client.
pub fn scan_local(dir: &Path) -> io::Result<DiskUsage> {
    // The canonical root is both the walk root and the volume anchor: one
    // resolution up front, so a symlinked request directory is scanned and
    // measured on its target's volume, never on the link's.
    let canonical = dir.canonicalize()?;
    let mut walk = Walk::default();
    walk.scan(&canonical)?;
    let scan = DirectoryScan::new(
        canonical.to_string_lossy().into_owned(),
        walk.size_bytes,
        walk.files_counted,
        volume_total(&canonical),
        walk.partial,
        walk.errors,
        now_millis(),
    );
    Ok(DiskUsage::new(
        dir.to_string_lossy().into_owned(),
        lyrel_protocol::daemon::control::server_instance(),
        DiskState::Scanned(scan),
    ))
}

#[derive(Default)]
struct Walk {
    budget: u64,
    size_bytes: u64,
    files_counted: u64,
    partial: bool,
    errors: Vec<String>,
}

impl Walk {
    /// Visit every entry under `root` without following links. An unreadable
    /// `root` is the scan's hard failure — there is no floor to report; an
    /// unreadable subtree is a partial result with the error carried along.
    fn scan(&mut self, root: &Path) -> io::Result<()> {
        self.scan_with_budget(root, MAX_ENTRIES)
    }

    fn scan_with_budget(&mut self, root: &Path, budget: u64) -> io::Result<()> {
        self.budget = budget;
        self.size_bytes = 0;
        self.files_counted = 0;
        self.partial = false;
        self.errors.clear();
        // The root must be readable or there is no floor to report at all.
        let first = fs::read_dir(root).map_err(|err| {
            io::Error::new(err.kind(), format!("cannot read {}: {err}", root.display()))
        })?;
        drop(first);
        // Directories open lazily by path, one enumeration at a time: a wide
        // tree never holds more handles than the pending stack is deep, and
        // dropping the rest on an early return closes nothing explicitly.
        let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let read = match fs::read_dir(&dir) {
                Ok(read) => read,
                Err(err) => {
                    self.note_error(&dir, &err);
                    continue;
                }
            };
            let mut nested: Vec<PathBuf> = Vec::new();
            for entry in read {
                if !self.spend() {
                    return Ok(());
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        self.note_error("entry", &err);
                        continue;
                    }
                };
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(err) => {
                        self.note_error(&path, &err);
                        continue;
                    }
                };
                // Never follow links: a symlink or junction counts as one
                // entry — its target, on any volume, is never visited.
                if file_type.is_symlink() || is_reparse_point(&entry) {
                    continue;
                }
                if file_type.is_dir() {
                    nested.push(path);
                } else {
                    match entry.metadata() {
                        Ok(meta) => {
                            self.size_bytes = self.size_bytes.saturating_add(meta.len());
                            self.files_counted = self.files_counted.saturating_add(1);
                        }
                        Err(err) => self.note_error(&path, &err),
                    }
                }
            }
            pending.extend(nested);
        }
        Ok(())
    }

    /// One entry off the budget; `false` means the ceiling is spent and the
    /// walk must stop, reporting a partial total.
    fn spend(&mut self) -> bool {
        if self.budget == 0 {
            self.partial = true;
            return false;
        }
        self.budget -= 1;
        true
    }

    fn note_error(&mut self, path: impl AsRef<Path>, err: &io::Error) {
        self.partial = true;
        if self.errors.len() < MAX_ERRORS {
            self.errors
                .push(format!("{}: {err}", path.as_ref().display()));
        }
    }
}

/// Junctions and other reparse points are directory links `file_type` does
/// not call symlinks; treat every reparse point conservatively at the same
/// safety boundary as the file tree uses.
#[cfg(windows)]
fn is_reparse_point(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    entry
        .metadata()
        .is_ok_and(|md| md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn is_reparse_point(_: &fs::DirEntry) -> bool {
    false
}

/// Measure the filesystem containing the canonical scan root. Windows accepts
/// a directory and handles mounted volumes and UNC paths.
#[cfg(windows)]
fn volume_total(canonical: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut path: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    if path.last() != Some(&(b'\\' as u16)) {
        path.push(b'\\' as u16);
    }
    path.push(0);
    let mut total = 0;
    // SAFETY: `path` is NUL-terminated and the only non-null output points to a u64.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            std::ptr::null_mut(),
            &mut total,
            std::ptr::null_mut(),
        )
    };
    (ok != 0 && total > 0).then_some(total)
}

#[cfg(unix)]
fn volume_total(canonical: &Path) -> Option<u64> {
    let path = std::ffi::CString::new(canonical.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is NUL-terminated and `statvfs` writes `stats` on success.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `statvfs` succeeded.
    let stats = unsafe { stats.assume_init() };
    let total = u64::try_from(u128::from(stats.f_blocks) * u128::from(stats.f_frsize)).ok()?;
    (total > 0).then_some(total)
}

#[cfg(not(any(unix, windows)))]
fn volume_total(_: &Path) -> Option<u64> {
    None
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn file(dir: &Path, name: &str, bytes: &[u8]) {
        let mut f = fs::File::create(dir.join(name)).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn scan(dir: &Path) -> DiskUsage {
        scan_local(dir).unwrap()
    }

    fn scanned(dir: &Path) -> DirectoryScan {
        match scan(dir).state() {
            DiskState::Scanned(scan) => scan.clone(),
            other => panic!("expected a completed scan, got {other:?}"),
        }
    }

    #[test]
    fn counts_nested_files_and_reports_the_requested_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("a/b")).unwrap();
        file(&root, "top.bin", &[0u8; 100]);
        file(&root.join("a"), "mid.bin", &[0u8; 50]);
        file(&root.join("a/b"), "deep.bin", &[0u8; 10]);

        let usage = scan(&root);
        // The answer is fenced by the requested path; the payload names the
        // exact directory the host scanned.
        assert_eq!(usage.directory(), root.to_str().unwrap());
        let scan = match usage.state() {
            DiskState::Scanned(scan) => scan.clone(),
            other => panic!("expected a completed scan, got {other:?}"),
        };
        assert_eq!(scan.directory(), root.to_str().unwrap());
        assert_eq!(scan.size_bytes(), 160);
        assert_eq!(scan.files_counted(), 3);
        assert!(!scan.is_partial());
        assert!(scan.errors().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_into_their_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.join("in")).unwrap();
        file(&root, "own.bin", &[0u8; 20]);
        file(outside.path(), "outside.bin", &[0u8; 500]);
        std::os::unix::fs::symlink(outside.path().join("outside.bin"), root.join("link.bin"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("in").join("dir-link")).unwrap();

        let scan = scanned(&root);
        // The 500-byte target file stays outside the total: neither the
        // file link nor the directory link was followed, and both links are
        // one unexplored entry each — nothing of the target counts.
        assert!(scan.size_bytes() < 500, "the symlink target must not count");
        assert_eq!(scan.size_bytes(), 20, "only the tree's own file counts");
        assert_eq!(scan.files_counted(), 1);
        assert!(!scan.is_partial());
    }

    #[test]
    fn an_unreadable_subdirectory_marks_the_scan_partial_with_the_error() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("locked")).unwrap();
        file(&root, "open.bin", &[0u8; 7]);

        // Deny read on the subtree where the platform supports it. Windows
        // ACLs would need token changes; Unix mode bits are enough there.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        }

        let usage = scan_local(&root).unwrap();
        let scan = match usage.state() {
            DiskState::Scanned(scan) => scan.clone(),
            other => panic!("expected a completed scan, got {other:?}"),
        };
        // The readable file still counts; the unreadable subtree is carried
        // as an explicit partial error rather than vanishing.
        assert!(scan.size_bytes() >= 7);
        #[cfg(unix)]
        {
            assert!(scan.is_partial());
            assert_eq!(scan.errors().len(), 1);
            assert!(scan.errors()[0].contains("locked"));
        }
        #[cfg(windows)]
        assert!(!scan.is_partial(), "no subtree was made unreadable here");
        let _ = usage;
    }

    #[test]
    fn a_missing_root_fails_instead_of_reading_as_an_empty_scan() {
        let temp = tempfile::tempdir().unwrap();
        let err = scan_local(&temp.path().join("gone")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn the_containing_volume_reports_a_real_total() {
        let temp = tempfile::tempdir().unwrap();
        let scan = scanned(&temp.path().canonicalize().unwrap());
        assert!(scan.volume_total_bytes().is_some_and(|total| total > 0));
    }

    #[test]
    fn scan_result_is_partial_once_the_entry_budget_is_spent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        // More files than a tiny budget admits, so the walk must stop early.
        for i in 0..64 {
            file(&root, &format!("f{i}.bin"), &[0u8; 1]);
        }
        let mut walk = Walk::default();
        walk.scan_with_budget(&root, 4).unwrap();
        assert!(walk.partial, "the spent budget marks the total a floor");
        assert!(walk.files_counted < 64);
    }
}
