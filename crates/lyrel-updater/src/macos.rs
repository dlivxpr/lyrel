use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::{log_line, next_path, next_pid, next_string, reject_extra, report_outcome};

const PARENT_POLL: Duration = Duration::from_millis(100);
const LAUNCH_GRACE: Duration = Duration::from_secs(1);

pub fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let command = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .ok_or_else(usage)?;
    match command.as_str() {
        "verify" => {
            let plan = VerifyPlan::parse(&mut args)?;
            verify_archive(&plan.archive, &plan.checksums, &plan.asset_name)?;
            let replacement = extract_archive(&plan.archive, &plan.stage)?;
            verify_update(&plan.current, &replacement, &plan.expected_version)
        }
        "install" => install(InstallPlan::parse(args)?),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: lyrel-updater verify <current.app> <archive.zip> <checksums.txt> \
     <asset-name> <stage-dir> <version>\n\
     or: lyrel-updater install <parent-pid> <current.app> <archive.zip> <checksums.txt> \
     <asset-name> <stage-dir> <version> <log-path> \
     [--config-dir <dir>] [--result-file <path>]"
        .to_string()
}

/// The pre-flight half of an update: verify the downloaded archive against
/// the release checksums, unpack it, and check the staged bundle before the
/// GUI commits to quitting. The GUI builds this plan and this executable
/// parses it — one argv layout, owned by this crate in both directions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyPlan {
    pub current: PathBuf,
    pub archive: PathBuf,
    pub checksums: PathBuf,
    pub asset_name: String,
    pub stage: PathBuf,
    pub expected_version: String,
}

impl VerifyPlan {
    /// `verify <current.app> <archive.zip> <checksums.txt> <asset-name> <stage-dir> <version>`
    pub fn argv(&self) -> Vec<OsString> {
        vec![
            OsString::from("verify"),
            self.current.as_os_str().to_os_string(),
            self.archive.as_os_str().to_os_string(),
            self.checksums.as_os_str().to_os_string(),
            OsString::from(&self.asset_name),
            self.stage.as_os_str().to_os_string(),
            OsString::from(&self.expected_version),
        ]
    }

    pub fn parse(args: &mut impl Iterator<Item = OsString>) -> Result<Self, String> {
        let plan = Self {
            current: next_path(args, usage)?,
            archive: next_path(args, usage)?,
            checksums: next_path(args, usage)?,
            asset_name: next_string(args, usage)?,
            stage: next_path(args, usage)?,
            expected_version: next_string(args, usage)?,
        };
        reject_extra(args, usage)?;
        Ok(plan)
    }
}

/// The install half. The GUI persists the fields — minus the parent pid,
/// which a later launch supplies fresh — and spawns this executable with
/// [`InstallPlan::argv`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    pub parent_pid: u32,
    pub current: PathBuf,
    pub archive: PathBuf,
    pub checksums: PathBuf,
    pub asset_name: String,
    pub stage: PathBuf,
    pub expected_version: String,
    pub log: PathBuf,
    pub config_dir: Option<PathBuf>,
    pub result_file: Option<PathBuf>,
}

impl InstallPlan {
    pub const COMMAND: &'static str = "install";

    /// The pre-flight view of this plan: same fields the `verify` verb
    /// reads before the GUI commits to quitting.
    pub fn verify_plan(&self) -> VerifyPlan {
        VerifyPlan {
            current: self.current.clone(),
            archive: self.archive.clone(),
            checksums: self.checksums.clone(),
            asset_name: self.asset_name.clone(),
            stage: self.stage.clone(),
            expected_version: self.expected_version.clone(),
        }
    }

    /// The seven positional arguments after the verb and the parent pid: the
    /// shape the GUI persists in its on-disk plan.
    pub fn rest(&self) -> Vec<PathBuf> {
        vec![
            self.current.clone(),
            self.archive.clone(),
            self.checksums.clone(),
            PathBuf::from(&self.asset_name),
            self.stage.clone(),
            PathBuf::from(&self.expected_version),
            self.log.clone(),
        ]
    }

    /// `install <parent-pid> <current.app> <archive.zip> <checksums.txt> <asset-name> <stage-dir> <version> <log-path> [--config-dir <dir>] [--result-file <path>]`
    pub fn argv(&self) -> Vec<OsString> {
        let rest = self.rest();
        let mut argv = Vec::with_capacity(rest.len() + 5);
        argv.push(OsString::from(Self::COMMAND));
        argv.push(OsString::from(self.parent_pid.to_string()));
        argv.extend(rest.iter().map(|arg| arg.as_os_str().to_os_string()));
        argv.extend(
            crate::TailOptions {
                config_dir: self.config_dir.clone(),
                result_file: self.result_file.clone(),
            }
            .argv(),
        );
        argv
    }

    pub fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let parent_pid = next_pid(&mut args, usage)?;
        let current = next_path(&mut args, usage)?;
        let archive = next_path(&mut args, usage)?;
        let checksums = next_path(&mut args, usage)?;
        let asset_name = next_string(&mut args, usage)?;
        let stage = next_path(&mut args, usage)?;
        let expected_version = next_string(&mut args, usage)?;
        let log = next_path(&mut args, usage)?;
        let mut tail = crate::TailOptions::default();
        while let Some(flag) = args.next() {
            let Some(flag) = flag.to_str() else {
                return Err(usage());
            };
            if !tail.parse_flag(flag, &mut args, usage)? {
                return Err(usage());
            }
        }
        tail.apply();
        Ok(Self {
            parent_pid,
            current,
            archive,
            checksums,
            asset_name,
            stage,
            expected_version,
            log,
            config_dir: tail.config_dir,
            result_file: tail.result_file,
        })
    }
}

fn install(plan: InstallPlan) -> Result<(), String> {
    install_inner(&plan)
}

fn install_inner(plan: &InstallPlan) -> Result<(), String> {
    let replacement = plan.stage.join("unpacked/lyrel.app");
    wait_for_exit(plan.parent_pid);
    log_line(&plan.log, "re-verifying staged lyrel update");
    let verification = verify_archive(&plan.archive, &plan.checksums, &plan.asset_name)
        .and_then(|()| verify_update(&plan.current, &replacement, &plan.expected_version));
    if let Err(error) = verification {
        log_line(&plan.log, &error);
        let _ = fs::remove_dir_all(&plan.stage);
        let result = Err(error);
        // The outcome lands before the old app does: the relaunched GUI
        // merges it at startup, and a write afterward races that merge
        // (#540).
        report_outcome(
            plan.result_file.as_deref(),
            &plan.log,
            &plan.expected_version,
            &result,
        );
        let _ = launch_app(&plan.current);
        return result;
    }
    log_line(&plan.log, &format!("replacing {}", plan.current.display()));
    let report = |result: &Result<(), String>| {
        report_outcome(
            plan.result_file.as_deref(),
            &plan.log,
            &plan.expected_version,
            result,
        );
    };
    replace_and_relaunch(&plan.current, &replacement, &plan.stage, launch_app, report)
        .inspect_err(|error| log_line(&plan.log, error))
}

fn verify_archive(archive: &Path, checksums: &Path, asset_name: &str) -> Result<(), String> {
    let bytes =
        fs::read(archive).map_err(|error| format!("reading {}: {error}", archive.display()))?;
    let manifest = fs::read_to_string(checksums)
        .map_err(|error| format!("reading {}: {error}", checksums.display()))?;
    lyrel_core::daemon::install::checksums::verify(&manifest, asset_name, &bytes)
        .map_err(|error| error.to_string())
}

fn extract_archive(archive: &Path, stage: &Path) -> Result<PathBuf, String> {
    let unpacked = stage.join("unpacked");
    fs::create_dir(&unpacked)
        .map_err(|error| format!("creating {}: {error}", unpacked.display()))?;
    run_checked(
        Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(archive)
            .arg(&unpacked),
        "extracting the update archive",
    )?;
    Ok(unpacked.join("lyrel.app"))
}

fn verify_update(current: &Path, replacement: &Path, expected_version: &str) -> Result<(), String> {
    let executable = replacement.join("Contents/MacOS/lyrel-app");
    let updater = replacement.join("Contents/MacOS/lyrel-updater");
    if !replacement.is_dir() || !executable.is_file() || !updater.is_file() {
        return Err(
            "the staged bundle is missing lyrel-app or lyrel-updater under Contents/MacOS"
                .to_string(),
        );
    }
    let actual_version = bundle_version(replacement)?;
    if actual_version != expected_version {
        return Err(format!(
            "the staged app reports version {actual_version}, expected {expected_version}"
        ));
    }
    run_checked(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(replacement),
        "verifying the staged app's code signature",
    )?;
    let current_identity = signing_identity(current)?;
    let replacement_identity = signing_identity(replacement)?;
    if current_identity.identifier != replacement_identity.identifier {
        return Err(format!(
            "the staged app has a different code-signing identifier: current {:?}, staged {:?}",
            current_identity.identifier, replacement_identity.identifier
        ));
    }
    if current_identity.ad_hoc {
        // Ad-hoc signatures have no publisher identity; their synthesized
        // requirements pin a cdhash that changes on every release. The
        // authenticated release download and archive checksum remain the
        // trust boundary, with a sealed signature and stable bundle id.
        if !replacement_identity.ad_hoc {
            return Err("the staged app has a different code-signing posture".to_string());
        }
    } else {
        if replacement_identity.ad_hoc {
            return Err(
                "the staged ad-hoc app cannot satisfy the current app's code-signing requirement"
                    .to_string(),
            );
        }
        // A displayed requirement is a claim, not evidence. Ask codesign
        // to evaluate the installed identity against the replacement,
        // retaining its certificate/anchor constraints instead of merely
        // comparing the displayed requirement strings.
        run_checked(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "-R"])
                .arg(format!("={}", current_identity.requirement))
                .arg(replacement),
            "verifying the current app's code-signing requirement against the staged app",
        )?;
    }
    Ok(())
}

fn bundle_version(app: &Path) -> Result<String, String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .map_err(|error| format!("reading the staged app version: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "reading the staged app version: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// codesign prints synthesized ad-hoc requirements as `# designated =>`
/// and embedded requirements as `designated =>`, on either output stream.
fn designated_requirement(stdout: &str, stderr: &str) -> Option<String> {
    [stdout, stderr]
        .into_iter()
        .flat_map(str::lines)
        .find_map(|line| {
            line.strip_prefix("# ")
                .unwrap_or(line)
                .strip_prefix("designated => ")
                .filter(|requirement| !requirement.is_empty())
                .map(str::to_string)
        })
}

struct SigningIdentity {
    identifier: String,
    requirement: String,
    ad_hoc: bool,
}

fn signing_identity(app: &Path) -> Result<SigningIdentity, String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4", "-r-"])
        .arg(app)
        .output()
        .map_err(|error| {
            format!(
                "reading the code-signing requirement for {}: {error}",
                app.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "reading the code-signing requirement for {}: {}",
            app.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines = stdout.lines().chain(stderr.lines());
    let identifier = lines
        .clone()
        .find_map(|line| line.strip_prefix("Identifier="))
        .filter(|identifier| !identifier.is_empty())
        .ok_or_else(|| "codesign did not report a code-signing identifier".to_string())?;
    let requirement = designated_requirement(&stdout, &stderr)
        .ok_or_else(|| "codesign did not report a designated requirement".to_string())?;
    Ok(SigningIdentity {
        identifier: identifier.to_string(),
        requirement,
        ad_hoc: lines.any(|line| line == "Signature=adhoc"),
    })
}

fn replace_and_relaunch(
    current: &Path,
    replacement: &Path,
    stage: &Path,
    launch: impl Fn(&Path) -> Result<(), String>,
    report: impl Fn(&Result<(), String>),
) -> Result<(), String> {
    // The staging directory is a fresh TempDir created beside the current
    // bundle, so a backup here stays on the same filesystem without using a
    // predictable sibling path.  In particular, never delete a fixed-name
    // path beside the app: it may be a recovery copy left by an interrupted
    // update (or simply an unrelated user-owned path).
    let backup = stage.join("previous.app");
    if backup.exists() {
        let result = Err(format!(
            "the update staging backup already exists: {}",
            backup.display()
        ));
        report(&result);
        return result;
    }
    if let Err(error) = fs::rename(current, &backup) {
        let result = Err(format!("moving the current app aside: {error}"));
        report(&result);
        return result;
    }

    if let Err(error) = fs::rename(replacement, current) {
        let _ = fs::rename(&backup, current);
        let _ = fs::remove_dir_all(stage);
        let result = Err(format!("putting the staged app in place: {error}"));
        report(&result);
        return result;
    }

    match launch(current) {
        Ok(()) => {
            let _ = remove_path(&backup);
            let _ = fs::remove_dir_all(stage);
            let result = Ok(());
            report(&result);
            result
        }
        Err(error) => {
            let _ = remove_path(current);
            let (result, relaunch) = match fs::rename(&backup, current) {
                Ok(()) => {
                    let _ = fs::remove_dir_all(stage);
                    (Err(error), true)
                }
                Err(restore) => (
                    Err(format!("{error}; restoring the previous app: {restore}")),
                    false,
                ),
            };
            // The outcome lands before the old app does: the relaunched GUI
            // merges it at startup, and a write afterward races that merge
            // (#540).
            report(&result);
            if relaunch {
                let _ = launch(current);
            }
            result
        }
    }
}

fn launch_app(app: &Path) -> Result<(), String> {
    let executable = app.join("Contents/MacOS/lyrel-app");
    let mut child = Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("launching {}: {error}", executable.display()))?;
    healthy_after_grace(&mut child)
}

fn healthy_after_grace(child: &mut Child) -> Result<(), String> {
    thread::sleep(LAUNCH_GRACE);
    match child
        .try_wait()
        .map_err(|error| format!("checking the relaunched app: {error}"))?
    {
        None => Ok(()),
        Some(status) => Err(format!(
            "the relaunched app exited immediately with {status}"
        )),
    }
}

fn wait_for_exit(pid: u32) {
    // The updater is spawned directly by the app it waits for, so while
    // that app lives it *is* this process's parent, and the kernel
    // reparents us to launchd the moment it exits. Watching getppid() is
    // therefore immune to pid reuse, which `kill(pid, 0)` is not: a
    // recycled pid keeps answering 0 forever. (Windows solves the same
    // race by holding a process handle — see the windows module.)
    let pid = pid as libc::pid_t;
    if unsafe { libc::getppid() } == pid {
        while unsafe { libc::getppid() } == pid {
            thread::sleep(PARENT_POLL);
        }
        return;
    }
    // Not our parent — a hand-run updater. The polling fallback keeps
    // that invocation working, pid-reuse caveat and all.
    while process_alive(pid) {
        thread::sleep(PARENT_POLL);
    }
}

fn process_alive(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn remove_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("removing {}: {error}", path.display()))
}

fn run_checked(command: &mut Command, context: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{context}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keep a certificate-signed fixture alongside the ad-hoc bundles below:
    /// their embedded and synthesized requirements have different prefixes.
    #[test]
    fn the_designated_requirement_is_read_off_the_stream_codesign_uses() {
        let out = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-", "/bin/ls"])
            .output()
            .expect("codesign is part of macOS");
        assert!(out.status.success(), "codesign refused /bin/ls");

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let requirement = designated_requirement(&stdout, &stderr)
            .expect("codesign reports a designated requirement for /bin/ls");
        assert!(
            requirement.contains("identifier"),
            "a designated requirement names an identifier: {requirement:?}"
        );
    }

    /// Reading both streams is what keeps the choice above from being a
    /// guess about a future macOS.
    #[test]
    fn a_requirement_on_either_stream_is_found_and_neither_is_an_error() {
        let line = "designated => identifier \"com.example.app\" and anchor apple";
        let want = Some("identifier \"com.example.app\" and anchor apple".to_string());

        assert_eq!(designated_requirement(line, "Executable=/x"), want);
        assert_eq!(designated_requirement("Executable=/x", line), want);
        let synthesized = format!("# {line}");
        assert_eq!(designated_requirement(&synthesized, ""), want);
        assert_eq!(designated_requirement("", &synthesized), want);
        assert_eq!(designated_requirement("designated => ", ""), None);
        assert_eq!(designated_requirement("Executable=/x", ""), None);
        assert_eq!(designated_requirement("", ""), None);
    }

    fn signed_bundle(path: &Path, version: &str, identifier: &str) {
        fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        fs::write(
            path.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{identifier}</string>
<key>CFBundleExecutable</key><string>lyrel-app</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        for executable in ["lyrel-app", "lyrel-updater"] {
            fs::copy("/bin/ls", path.join("Contents/MacOS").join(executable)).unwrap();
        }
        run_checked(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(path),
            "ad-hoc signing the isolated test bundle",
        )
        .unwrap();
    }

    #[test]
    fn verify_accepts_an_ad_hoc_update_with_a_new_cdhash() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current.app");
        let replacement = root.path().join("replacement.app");
        signed_bundle(&current, "0.1.1", "io.github.dlivxpr.lyrel");
        signed_bundle(&replacement, "0.1.2", "io.github.dlivxpr.lyrel");

        let current_requirement = signing_identity(&current).unwrap().requirement;
        let replacement_requirement = signing_identity(&replacement).unwrap().requirement;
        assert_ne!(current_requirement, replacement_requirement);
        verify_update(&current, &replacement, "0.1.2").unwrap();
    }

    #[test]
    fn verify_rejects_an_ad_hoc_update_with_a_different_identifier() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current.app");
        let replacement = root.path().join("replacement.app");
        signed_bundle(&current, "0.1.1", "io.github.dlivxpr.lyrel");
        signed_bundle(&replacement, "0.1.2", "com.example.other");

        let error = verify_update(&current, &replacement, "0.1.2").unwrap_err();
        assert!(
            error.contains("different code-signing identifier"),
            "{error}"
        );
    }

    #[test]
    fn verify_rejects_a_corrupted_ad_hoc_update() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current.app");
        let replacement = root.path().join("replacement.app");
        signed_bundle(&current, "0.1.1", "io.github.dlivxpr.lyrel");
        signed_bundle(&replacement, "0.1.2", "io.github.dlivxpr.lyrel");
        fs::write(replacement.join("Contents/MacOS/lyrel-updater"), b"corrupt").unwrap();

        let error = verify_update(&current, &replacement, "0.1.2").unwrap_err();
        assert!(
            error.contains("verifying the staged app's code signature"),
            "{error}"
        );
    }

    #[test]
    fn verify_rejects_an_unsigned_update() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current.app");
        let replacement = root.path().join("replacement.app");
        signed_bundle(&current, "0.1.1", "io.github.dlivxpr.lyrel");
        signed_bundle(&replacement, "0.1.2", "io.github.dlivxpr.lyrel");
        run_checked(
            Command::new("/usr/bin/codesign")
                .arg("--remove-signature")
                .arg(&replacement),
            "removing the test bundle signature",
        )
        .unwrap();

        let error = verify_update(&current, &replacement, "0.1.2").unwrap_err();
        assert!(
            error.contains("verifying the staged app's code signature"),
            "{error}"
        );
    }

    #[test]
    fn verify_rejects_an_ad_hoc_update_of_a_certificate_signed_identity() {
        let root = tempfile::tempdir().unwrap();
        let replacement = root.path().join("replacement.app");
        signed_bundle(&replacement, "0.1.2", "com.apple.ls");
        // An explicit copied requirement is not proof of a signing identity.
        // Use Apple's existing signature without creating a test keychain.
        let requirement = signing_identity(Path::new("/bin/ls")).unwrap().requirement;
        run_checked(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-", "--requirements"])
                .arg(format!("=designated => {requirement}"))
                .arg(&replacement),
            "copying the certificate-signed identity's requirement",
        )
        .unwrap();

        let error = verify_update(Path::new("/bin/ls"), &replacement, "0.1.2").unwrap_err();
        assert!(
            error.contains("current app's code-signing requirement"),
            "{error}"
        );
    }

    fn bundle(path: &Path, marker: &str) {
        fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        fs::write(path.join("marker"), marker).unwrap();
    }

    #[test]
    fn successful_launch_commits_the_replacement() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("lyrel.app");
        let stage = root.path().join("stage");
        let replacement = stage.join("lyrel.app");
        bundle(&current, "old");
        bundle(&replacement, "new");

        replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

        assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "new");
        assert!(!stage.exists());
        assert!(!root.path().join(".lyrel.app.lyrel-update-backup").exists());
    }

    #[test]
    fn failed_launch_restores_and_relaunches_the_previous_app() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("lyrel.app");
        let stage = root.path().join("stage");
        let replacement = stage.join("lyrel.app");
        bundle(&current, "old");
        bundle(&replacement, "new");
        let launches = std::cell::Cell::new(0);
        let reported_after_launches = std::cell::Cell::new(usize::MAX);

        let error = replace_and_relaunch(
            &current,
            &replacement,
            &stage,
            |_| {
                launches.set(launches.get() + 1);
                if launches.get() == 1 {
                    Err("new app failed".to_string())
                } else {
                    Ok(())
                }
            },
            |_| reported_after_launches.set(launches.get()),
        )
        .unwrap_err();

        assert_eq!(error, "new app failed");
        assert_eq!(launches.get(), 2);
        // The outcome is reported after the failed first launch but before
        // the old app comes back — the relaunched GUI must find it already
        // on disk at startup (#540).
        assert_eq!(reported_after_launches.get(), 1);
        assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "old");
        assert!(!stage.exists());
    }

    #[test]
    fn replacement_does_not_remove_a_fixed_name_sibling() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("lyrel.app");
        let stage = root.path().join("stage");
        let replacement = stage.join("lyrel.app");
        let sibling = root.path().join(".lyrel.app.lyrel-update-backup");
        bundle(&current, "old");
        bundle(&replacement, "new");
        bundle(&sibling, "keep");
        let legacy = [
            root.path().join("otherterm.app"),
            root.path().join(".otherterm-update-preserved/previous.app"),
        ];
        for path in &legacy {
            bundle(path, "legacy");
        }

        replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

        assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "new");
        assert_eq!(fs::read_to_string(sibling.join("marker")).unwrap(), "keep");
        for path in &legacy {
            assert_eq!(fs::read_to_string(path.join("marker")).unwrap(), "legacy");
        }
    }

    #[test]
    fn verify_rejects_a_bundle_without_the_helper() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current.app");
        let replacement = root.path().join("replacement.app");
        bundle(&current, "old");
        fs::create_dir_all(replacement.join("Contents/MacOS")).unwrap();
        fs::write(replacement.join("Contents/MacOS/lyrel-app"), b"app").unwrap();

        let error = verify_update(&current, &replacement, "1.0.0").unwrap_err();
        assert!(
            error.contains("missing lyrel-app or lyrel-updater"),
            "{error}"
        );
    }

    #[test]
    fn archive_verification_rejects_bytes_that_do_not_match_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("lyrel.zip");
        let manifest = root.path().join("checksums.txt");
        fs::write(&archive, b"downloaded bytes").unwrap();
        fs::write(
            &manifest,
            format!(
                "{}  lyrel.zip\n",
                lyrel_core::daemon::install::checksums::hex(
                    &lyrel_core::daemon::install::checksums::sha256(b"published bytes")
                )
            ),
        )
        .unwrap();

        let error = verify_archive(&archive, &manifest, "lyrel.zip").unwrap_err();
        assert!(error.contains("failed sha256 verification"), "{error}");
    }

    #[test]
    fn bundle_version_preserves_the_complete_prerelease_identity() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("lyrel.app");
        let contents = app.join("Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
<key>CFBundleShortVersionString</key>
<string>1.2.3-rc.2</string>
</dict>
</plist>
"#,
        )
        .unwrap();

        assert_eq!(bundle_version(&app).unwrap(), "1.2.3-rc.2");
    }
}
