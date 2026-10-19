//! The installer against a fake Windows OpenSSH host.
//!
//! The fake answers the way such a host does where it matters: `uname` is not
//! a command, SFTP spells paths `/C:/…` and reports no execute bits, a rename
//! onto an existing file fails, and the running server's image cannot be
//! deleted. Every other command has to arrive as one of `windows_host`'s
//! encoded PowerShell scripts — anything else is recorded as foreign, and the
//! tests fail on it, because a POSIX command reaching `cmd.exe` is exactly the
//! bug this suite exists to catch.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::asset::{ASSET_WINDOWS_X86_64, CHECKSUMS_ASSET, RemotePlatform, UnsupportedTarget};
use super::*;

const VERSION: &str = "26.9.9";
const CONTROL: u32 = 3;
const PROTOCOL: u32 = 4;
const HOME: &str = "/C:/Users/me";
const BIN_DIR: &str = "/C:/Users/me/AppData/Local/lyrel/bin";
const BINARY: &str = "/C:/Users/me/AppData/Local/lyrel/bin/lyrel-server-c3p4.exe";
const SERVER_BYTES: &[u8] = b"MZ\x90\x00...a lyrel-server.exe, pretend it is 8 MB";
const CONPTY_DLL_BYTES: &[u8] = b"MZ\x90\x00...the pinned conpty.dll";
const OPENCONSOLE_BYTES: &[u8] = b"MZ\x90\x00...the pinned OpenConsole.exe";
const CONPTY_LICENSE_BYTES: &[u8] = b"the MIT notice of the pinned ConPTY package";
const LICENSE_BYTES: &[u8] = b"the Apache-2.0 notice of lyrel";

/// A ZIP of `entries`, laid out the way the release workflow's
/// `Compress-Archive` lays out the real one: flat names, stored bytes.
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    smol::block_on(async {
        let mut output = Vec::new();
        {
            let mut writer = async_zip::base::write::ZipFileWriter::new(&mut output);
            for (name, bytes) in entries {
                let options =
                    async_zip::ZipEntryBuilder::new((*name).into(), async_zip::Compression::Stored);
                writer.write_entry_whole(options, bytes).await.unwrap();
            }
            writer.close().await.unwrap();
        }
        output
    })
}

/// The published Windows server package: the server and the pinned ConPTY
/// set, in [`asset`]'s layout order.
fn server_zip() -> Vec<u8> {
    zip_bytes(&[
        (asset::WINDOWS_PACKAGE_EXE, SERVER_BYTES),
        ("conpty.dll", CONPTY_DLL_BYTES),
        ("OpenConsole.exe", OPENCONSOLE_BYTES),
        ("LICENSE-ConPTY.txt", CONPTY_LICENSE_BYTES),
        ("LICENSE.txt", LICENSE_BYTES),
    ])
}

fn companion_bytes(name: &str) -> &'static [u8] {
    match name {
        "conpty.dll" => CONPTY_DLL_BYTES,
        "OpenConsole.exe" => OPENCONSOLE_BYTES,
        "LICENSE-ConPTY.txt" => CONPTY_LICENSE_BYTES,
        "LICENSE.txt" => LICENSE_BYTES,
        other => panic!("{other} is not a package companion"),
    }
}

fn ours() -> RemoteProtocol {
    RemoteProtocol {
        control: CONTROL,
        protocol: PROTOCOL,
        build: VERSION.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// `uname -sm`, which the fake refuses the way cmd.exe does.
    Uname,
    /// A `windows_host` script, by its tag.
    Script(String),
    Mkdir(String),
    Chmod(String),
    Put(String),
    Rename {
        from: String,
        to: String,
    },
    Remove(String),
    /// Anything a Windows shell would not have understood.
    Foreign(String),
}

struct FakeWindows {
    home: String,
    /// `None` is cmd.exe or PowerShell: no `uname` at all.
    uname: Option<String>,
    /// `None`: PowerShell does not answer either (not Windows after all).
    arch: Option<String>,
    files: Mutex<HashMap<String, (Vec<u8>, bool)>>,
    speaks: Mutex<HashMap<String, RemoteProtocol>>,
    uploads_speak: RemoteProtocol,
    /// The image of the running daemon, in SFTP spelling.
    running: Mutex<Option<String>>,
    steps: Mutex<Vec<Step>>,
    probe_rename_failures: usize,
    probe_rename_error: String,
    source_rename_failures: Mutex<usize>,
}

impl FakeWindows {
    fn new(arch: &str) -> Self {
        let mut files = HashMap::new();
        files.insert(HOME.to_string(), (Vec::new(), true));
        Self {
            home: HOME.to_string(),
            uname: None,
            arch: Some(arch.to_string()),
            files: Mutex::new(files),
            speaks: Mutex::new(HashMap::new()),
            uploads_speak: ours(),
            running: Mutex::new(None),
            steps: Mutex::new(Vec::new()),
            probe_rename_failures: 0,
            probe_rename_error: std::io::Error::from_raw_os_error(32).to_string(),
            source_rename_failures: Mutex::new(0),
        }
    }

    fn with_uname(mut self, uname: &str) -> Self {
        self.uname = Some(uname.to_string());
        self
    }

    fn not_answering_powershell(mut self) -> Self {
        self.arch = None;
        self
    }

    fn installed(self, path: &str, spoken: RemoteProtocol) -> Self {
        {
            let mut files = self.files.lock().unwrap();
            for dir in
                asset::remote_paths_on(RemotePlatform::Windows, HOME, CONTROL, PROTOCOL).dir_chain
            {
                files.insert(dir, (Vec::new(), true));
            }
            files.insert(path.to_string(), (SERVER_BYTES.to_vec(), false));
            // A real install is the server and the pinned set beside it.
            let dir = path.rsplit_once('/').expect("an install path").0;
            for name in asset::WINDOWS_PACKAGE_COMPANIONS {
                files.insert(
                    format!("{dir}/{name}"),
                    (companion_bytes(name).to_vec(), false),
                );
            }
        }
        self.speaks.lock().unwrap().insert(path.to_string(), spoken);
        self
    }

    /// A file a complete install would have is gone — the state a bare-exe
    /// install or an incomplete ConPTY set leaves behind.
    fn without(self, path: &str) -> Self {
        self.files.lock().unwrap().remove(path);
        self
    }

    fn uploads_speaking(mut self, spoken: RemoteProtocol) -> Self {
        self.uploads_speak = spoken;
        self
    }

    fn serving(self, image: &str) -> Self {
        *self.running.lock().unwrap() = Some(image.to_string());
        self
    }

    fn steps(&self) -> Vec<Step> {
        self.steps.lock().unwrap().clone()
    }

    fn scripts(&self) -> Vec<String> {
        self.steps()
            .into_iter()
            .filter_map(|s| match s {
                Step::Script(tag) => Some(tag),
                _ => None,
            })
            .collect()
    }

    fn assert_nothing_foreign(&self) {
        let foreign: Vec<Step> = self
            .steps()
            .into_iter()
            .filter(|s| matches!(s, Step::Foreign(_)))
            .collect();
        assert!(
            foreign.is_empty(),
            "a Windows host was sent commands no Windows shell runs: {foreign:?}"
        );
    }

    fn has(&self, path: &str) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }

    fn step(&self, step: Step) {
        self.steps.lock().unwrap().push(step);
    }
}

fn ok(stdout: &str) -> Result<ExecOutput, String> {
    Ok(ExecOutput {
        status: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

fn failed(status: u32, stderr: &str) -> Result<ExecOutput, String> {
    Ok(ExecOutput {
        status: Some(status),
        stdout: String::new(),
        stderr: stderr.to_string(),
    })
}

/// The native path a script names as `& '<path>' <flag>`, back in SFTP form.
fn invoked(script: &str, flag: &str) -> Option<String> {
    let (before, _) = script.split_once(&format!("' {flag}"))?;
    let native = before.rsplit_once("& '")?.1;
    asset::sftp_path_from_windows(native)
}

impl RemoteOps for FakeWindows {
    fn home_dir(&self) -> Result<String, String> {
        Ok(self.home.clone())
    }

    fn run(&self, cmd: &str) -> Result<ExecOutput, String> {
        if cmd == "uname -sm" {
            self.step(Step::Uname);
            return match &self.uname {
                Some(uname) => ok(uname),
                None => failed(
                    1,
                    "'uname' is not recognized as an internal or external command,\r\n\
                     operable program or batch file.\r\n",
                ),
            };
        }
        let Some((tag, script)) = windows_host::decode(cmd) else {
            self.step(Step::Foreign(cmd.to_string()));
            return failed(1, "is not recognized as an internal or external command");
        };
        self.step(Step::Script(tag.clone()));
        match tag.as_str() {
            "probe" => match &self.arch {
                Some(arch) => ok(&format!("{} {arch}\r\n", windows_host::PROBE_MARK)),
                None => Err("powershell.exe: command not found".into()),
            },
            "protocol" => {
                let exe = invoked(&script, PROTOCOL_FLAG).expect("a native path to probe");
                if exe.ends_with(".tmp.exe") {
                    *self.source_rename_failures.lock().unwrap() = self.probe_rename_failures;
                }
                match self.speaks.lock().unwrap().get(&exe) {
                    Some(spoken) => ok(&format!("{}\r\n", spoken.to_line())),
                    None => failed(1, "The term is not recognized"),
                }
            }
            "control-probe" => match self.running.lock().unwrap().is_some() {
                true => ok(""),
                false => failed(
                    1,
                    "lyrel-server: stdio session ended with error: no daemon port file",
                ),
            },
            "running-exe" => ok(&self
                .running
                .lock()
                .unwrap()
                .as_deref()
                .and_then(asset::windows_native_path)
                .unwrap_or_default()),
            "stop" => {
                assert!(
                    invoked(&script, "--stop").is_some(),
                    "the stop goes through a server binary: {script}"
                );
                *self.running.lock().unwrap() = None;
                ok("")
            }
            "launch" => {
                assert!(
                    script.contains("Win32_Process -MethodName Create"),
                    "{script}"
                );
                *self.running.lock().unwrap() = Some(BINARY.to_string());
                ok("")
            }
            "read-exit" | "tail" => ok(""),
            other => panic!("the fake does not know the `{other}` script"),
        }
    }

    fn spawn_detached(&self, cmd: &str) -> Result<(), String> {
        self.run(cmd).map(|_| ())
    }

    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, String> {
        // Windows OpenSSH reports no execute bits for an .exe.
        Ok(self
            .files
            .lock()
            .unwrap()
            .get(path)
            .map(|(bytes, is_dir)| RemoteStat {
                size: bytes.len() as u64,
                mode: if *is_dir { 0o40755 } else { 0o100644 },
                is_dir: *is_dir,
            }))
    }

    fn mkdir(&self, path: &str) -> Result<(), String> {
        self.step(Step::Mkdir(path.to_string()));
        assert!(asset::is_windows_sftp_path(path), "{path}");
        self.files
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_insert((Vec::new(), true));
        Ok(())
    }

    fn chmod(&self, path: &str, _mode: u32) -> Result<(), String> {
        self.step(Step::Chmod(path.to_string()));
        Ok(())
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.step(Step::Put(path.to_string()));
        assert!(
            path.starts_with(&format!("{BIN_DIR}/.")),
            "an upload lands on a hidden temp name beside the server: {path}"
        );
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), (bytes.to_vec(), false));
        self.speaks
            .lock()
            .unwrap()
            .insert(path.to_string(), self.uploads_speak.clone());
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        self.step(Step::Rename {
            from: from.to_string(),
            to: to.to_string(),
        });
        if from.ends_with(".tmp.exe") {
            let mut remaining = self.source_rename_failures.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(self.probe_rename_error.clone());
            }
        }
        let mut files = self.files.lock().unwrap();
        // Windows OpenSSH renames without replacing.
        if files.contains_key(to) {
            return Err("4: Failure".into());
        }
        let file = files.remove(from).ok_or("2: No such file")?;
        files.insert(to.to_string(), file);
        let mut speaks = self.speaks.lock().unwrap();
        if let Some(spoken) = speaks.remove(from) {
            speaks.insert(to.to_string(), spoken);
        }
        // A running image keeps running from wherever it is moved to.
        let mut running = self.running.lock().unwrap();
        if running.as_deref() == Some(from) {
            *running = Some(to.to_string());
        }
        Ok(())
    }

    fn remove_file(&self, path: &str) -> Result<(), String> {
        self.step(Step::Remove(path.to_string()));
        if self.running.lock().unwrap().as_deref() == Some(path) {
            return Err("4: Failure (the image is in use)".into());
        }
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn list_dir(&self, path: &str) -> Result<Option<Vec<String>>, String> {
        let files = self.files.lock().unwrap();
        if !files.get(path).is_some_and(|(_, dir)| *dir) {
            return Ok(None);
        }
        let prefix = format!("{path}/");
        Ok(Some(
            files
                .keys()
                .filter_map(|k| k.strip_prefix(&prefix))
                .filter(|rest| !rest.contains('/'))
                .map(str::to_string)
                .collect(),
        ))
    }
}

struct Release {
    fetched: Mutex<Vec<String>>,
    zip: Vec<u8>,
}

impl Release {
    fn new() -> Self {
        Self::with_zip(server_zip())
    }

    fn with_zip(zip: Vec<u8>) -> Self {
        Self {
            fetched: Mutex::new(Vec::new()),
            zip,
        }
    }
}

impl AssetFetcher for Release {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.fetched.lock().unwrap().push(url.to_string());
        let digest = checksums::hex(&checksums::sha256(&self.zip));
        if url.ends_with(CHECKSUMS_ASSET) {
            return Ok(format!("{digest}  {ASSET_WINDOWS_X86_64}\n").into_bytes());
        }
        if url.ends_with(ASSET_WINDOWS_X86_64) {
            return Ok(self.zip.clone());
        }
        Err(format!("404: {url}"))
    }
}

struct Approve(Mutex<Vec<InstallRequest>>);

impl InstallConfirm for Approve {
    fn confirm(&self, request: &InstallRequest) -> InstallDecision {
        self.0.lock().unwrap().push(request.clone());
        InstallDecision::Approve
    }
}

fn installer<'a>(host: &'a FakeWindows, release: &'a Release, user: &'a Approve) -> Installer<'a> {
    Installer::new(host, release, user, "win-box")
        .with_version(VERSION)
        .with_dialect(CONTROL, PROTOCOL)
        .with_timeouts(Duration::from_millis(200), Duration::from_millis(10))
        .with_shutdown_timeout(Duration::from_millis(200))
}

#[test]
fn a_fresh_windows_host_gets_the_windows_server_in_local_app_data() {
    let host = FakeWindows::new("AMD64");
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));

    let report = installer(&host, &release, &user)
        .run()
        .expect("a Windows host installs");

    host.assert_nothing_foreign();
    assert_eq!(report.asset, ASSET_WINDOWS_X86_64);
    assert_eq!(report.paths.platform, RemotePlatform::Windows);
    assert_eq!(report.paths.binary, BINARY);
    assert!(report.installed && report.confirmed && report.launched);
    assert_eq!(
        host.files
            .lock()
            .unwrap()
            .get(BINARY)
            .map(|(bytes, _)| bytes.as_slice()),
        Some(SERVER_BYTES),
        "the server took its dialect name"
    );
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        assert_eq!(
            host.files
                .lock()
                .unwrap()
                .get(&format!("{BIN_DIR}/{name}"))
                .map(|(bytes, _)| bytes.as_slice()),
            Some(companion_bytes(name)),
            "the pinned set landed beside it: {name}"
        );
    }

    let asked = user.0.lock().unwrap();
    assert_eq!(asked.len(), 1, "a first install is confirmed");
    assert_eq!(asked[0].remote_path, BINARY);
    assert_eq!(asked[0].asset, ASSET_WINDOWS_X86_64);
    assert_eq!(
        asked[0].size_bytes,
        server_zip().len() as u64,
        "the confirmation describes the downloaded package"
    );
    assert_eq!(
        asked[0].sha256,
        checksums::hex(&checksums::sha256(&server_zip()))
    );
    assert!(
        release.fetched.lock().unwrap()[1].ends_with("/v26.9.9/lyrel-server-windows-x86_64.zip"),
        "{:?}",
        release.fetched.lock().unwrap()
    );

    let steps = host.steps();
    assert_eq!(steps[0], Step::Uname, "unix is still asked first");
    assert_eq!(steps[1], Step::Script("probe".into()));
    assert!(
        !steps.iter().any(|s| matches!(s, Step::Chmod(_))),
        "NTFS has no mode bits to set: {steps:?}"
    );
    for dir in [
        "/C:/Users/me/AppData",
        "/C:/Users/me/AppData/Local",
        "/C:/Users/me/AppData/Local/lyrel",
        BIN_DIR,
    ] {
        assert!(steps.contains(&Step::Mkdir(dir.into())), "{dir}: {steps:?}");
    }
    let puts: Vec<String> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Put(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        puts.len(),
        5,
        "the server and its four companions go up: {puts:?}"
    );
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        let temp = puts
            .iter()
            .find(|p| p.starts_with(&format!("{BIN_DIR}/.{name}.")) && p.ends_with(".tmp"))
            .unwrap_or_else(|| panic!("an upload for {name}: {puts:?}"));
        assert!(
            steps.contains(&Step::Rename {
                from: temp.clone(),
                to: format!("{BIN_DIR}/{name}"),
            }),
            "the {name} upload took its final name: {steps:?}"
        );
    }
    let exe_temp = puts
        .iter()
        .find(|p| {
            p.starts_with(&format!("{BIN_DIR}/.lyrel-server-c3p4.")) && p.ends_with(".tmp.exe")
        })
        .expect("the server uploads last")
        .clone();
    assert!(
        puts.last() == Some(&exe_temp),
        "the executable a probe judges the install by moves last: {puts:?}"
    );
    assert!(steps.contains(&Step::Rename {
        from: exe_temp,
        to: BINARY.into()
    }));

    let scripts = host.scripts();
    let launch = scripts
        .iter()
        .position(|t| t == "launch")
        .expect("a launch");
    let protocol = scripts
        .iter()
        .position(|t| t == "protocol")
        .expect("a probe");
    assert!(
        protocol < launch,
        "the upload is proven before it is started: {scripts:?}"
    );
    assert_eq!(scripts.last().map(String::as_str), Some("running-exe"));
}

fn source_publications(host: &FakeWindows) -> usize {
    host.steps()
        .iter()
        .filter(|step| {
            matches!(step, Step::Rename { from, to }
            if from.ends_with(".tmp.exe") && to == BINARY)
        })
        .count()
}

fn sftp_status_error(code: russh_sftp::protocol::StatusCode, message: &str) -> String {
    russh_sftp::client::error::Error::Status(russh_sftp::protocol::Status {
        id: 1,
        status_code: code,
        error_message: message.into(),
        language_tag: String::new(),
    })
    .to_string()
}

#[test]
fn probed_image_locks_are_retried_on_first_install_and_forced_update() {
    let errors = [
        std::io::Error::from_raw_os_error(32).to_string(),
        std::io::Error::from_raw_os_error(5).to_string(),
        sftp_status_error(russh_sftp::protocol::StatusCode::Failure, "Failure"),
    ];
    for error in errors {
        for update in [false, true] {
            let mut host = FakeWindows::new("AMD64");
            if update {
                host = host.installed(BINARY, ours()).serving(BINARY);
            }
            host.probe_rename_failures = 3;
            host.probe_rename_error = error.clone();
            let release = Release::new();
            let user = Approve(Default::default());
            let install = installer(&host, &release, &user);
            if update {
                install.replace_forced().expect("the source lock clears");
            } else {
                let report = install.run().expect("the source lock clears");
                assert!(report.installed && report.launched);
            }
            assert!(host.has(BINARY));
            assert_eq!(source_publications(&host), 4);
            assert_eq!(*host.source_rename_failures.lock().unwrap(), 0);
            host.assert_nothing_foreign();
        }
    }
}

#[test]
fn persistent_image_locks_stop_before_launching_an_unpublished_server() {
    let mut host = FakeWindows::new("AMD64");
    host.probe_rename_failures = usize::MAX;
    let release = Release::new();
    let user = Approve(Default::default());
    let error = installer(&host, &release, &user).run().unwrap_err();
    assert!(
        matches!(&error, InstallError::Write { path, reason }
        if path == BINARY && reason == &host.probe_rename_error),
        "{error:?}"
    );
    assert!((2..=21).contains(&source_publications(&host)));
    assert!(!host.has(BINARY));
    assert!(!host.scripts().iter().any(|tag| tag == "launch"));
}

#[test]
fn permanent_image_publication_errors_are_not_retried_as_locks() {
    let errors = [
        std::io::Error::from_raw_os_error(2).to_string(),
        std::io::Error::from_raw_os_error(112).to_string(),
        sftp_status_error(
            russh_sftp::protocol::StatusCode::PermissionDenied,
            "Permission denied",
        ),
    ];
    for expected in errors {
        let mut host = FakeWindows::new("AMD64");
        host.probe_rename_failures = usize::MAX;
        host.probe_rename_error = expected.clone();
        let release = Release::new();
        let user = Approve(Default::default());
        let error = installer(&host, &release, &user).run().unwrap_err();
        assert!(
            matches!(&error, InstallError::Write { path, reason }
            if path == BINARY && reason == &expected),
            "{error:?}"
        );
        assert_eq!(source_publications(&host), 2);
        assert!(!host.scripts().iter().any(|tag| tag == "launch"));
    }
}

#[test]
fn an_arm64_windows_host_is_refused() {
    let host = FakeWindows::new("ARM64");
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let err = installer(&host, &release, &user).run().unwrap_err();
    assert!(
        matches!(
            err,
            InstallError::Unsupported(UnsupportedTarget::UnknownWindowsMachine { .. })
        ),
        "{err:?}"
    );
    assert!(release.fetched.lock().unwrap().is_empty());
    host.assert_nothing_foreign();
}

#[test]
fn a_32_bit_windows_is_refused_before_anything_is_written() {
    let host = FakeWindows::new("x86");
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let err = installer(&host, &release, &user).run().unwrap_err();
    assert!(
        matches!(
            err,
            InstallError::Unsupported(UnsupportedTarget::UnknownWindowsMachine { .. })
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("\"x86\""), "{err}");
    assert!(release.fetched.lock().unwrap().is_empty());
    assert!(!host.has(BIN_DIR));
}

#[test]
fn git_for_windows_on_the_path_still_gets_the_windows_server() {
    let host = FakeWindows::new("AMD64").with_uname("MINGW64_NT-10.0-26100 x86_64\n");
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let report = installer(&host, &release, &user).run().unwrap();
    assert_eq!(report.asset, ASSET_WINDOWS_X86_64);
    assert_eq!(report.paths.binary, BINARY);
    host.assert_nothing_foreign();
}

/// Neither probe answering is a machine that is neither, and `uname`'s
/// failure is the one worth reading.
#[test]
fn when_nothing_answers_the_uname_failure_is_reported() {
    let host = FakeWindows::new("AMD64").not_answering_powershell();
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let err = installer(&host, &release, &user).run().unwrap_err();
    match err {
        InstallError::Probe(reason) => {
            assert!(reason.contains("'uname' is not recognized"), "{reason}")
        }
        other => panic!("expected the uname failure, got {other:?}"),
    }
}

/// A Windows host whose OpenSSH `DefaultShell` is WSL's bash answers `uname`
/// as Linux while SFTP writes to `C:\`. A Linux server installed at `/C:/…`
/// would be a file no Linux path reaches.
#[test]
fn a_linux_shell_over_windows_sftp_is_refused() {
    let host = FakeWindows::new("AMD64").with_uname("Linux x86_64\n");
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let err = installer(&host, &release, &user).run().unwrap_err();
    let InstallError::Probe(reason) = &err else {
        panic!("expected a probe error, got {err:?}");
    };
    assert!(reason.contains("DefaultShell"), "{reason}");
    assert!(release.fetched.lock().unwrap().is_empty());
    assert!(!host.has(BIN_DIR), "nothing was written");
}

#[test]
fn a_serving_windows_host_is_left_alone() {
    let host = FakeWindows::new("AMD64")
        .installed(BINARY, ours())
        .serving(BINARY);
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let report = installer(&host, &release, &user).run().unwrap();
    assert!(!report.installed && !report.launched);
    assert_eq!(report.mismatch, None);
    assert!(
        release.fetched.lock().unwrap().is_empty(),
        "an .exe with no execute bit over SFTP is still an installed server"
    );
    host.assert_nothing_foreign();
}

/// The running image cannot be deleted on Windows, but it can be renamed, and
/// that is how an update gets its name back — for the server and for the
/// `conpty.dll` it has loaded alike.
#[test]
fn an_update_moves_the_running_image_aside_and_the_next_one_sweeps_it() {
    let host = FakeWindows::new("AMD64")
        .installed(BINARY, ours())
        .serving(BINARY);
    let old_resources = [
        "/C:/Users/me/AppData/Local/otherterm/bin/otherterm-server-c12p7.exe",
        "/C:/Users/me/AppData/Roaming/otherterm/identity.json",
        "/C:/Users/me/AppData/Local/otherterm/bin/otherterm-server-c12p7.exe.startup.log",
        "/C:/Users/me/AppData/Local/lyrel/bin/.otherterm-server-c12p7.exe.1.old",
    ];
    for path in old_resources {
        host.files
            .lock()
            .unwrap()
            .insert(path.into(), (b"old product resource".to_vec(), false));
    }
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));

    installer(&host, &release, &user)
        .replace_forced()
        .expect("the update goes through");
    host.assert_nothing_foreign();

    let steps = host.steps();
    let mut asides = Vec::new();
    for name in ["lyrel-server-c3p4.exe"]
        .into_iter()
        .chain(asset::WINDOWS_PACKAGE_COMPANIONS)
    {
        let aside = steps
            .iter()
            .find_map(|s| match s {
                Step::Rename { from, to } if from == &format!("{BIN_DIR}/{name}") => {
                    Some(to.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the old {name} was moved aside: {steps:?}"));
        assert!(
            aside.starts_with(&format!("{BIN_DIR}/.{name}.")) && aside.ends_with(".old"),
            "{aside}"
        );
        assert!(
            host.has(&aside),
            "the old {name} is still there until it stops being used"
        );
        asides.push(aside);
    }
    assert!(host.has(BINARY), "the new server took the name");
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        assert!(host.has(&format!("{BIN_DIR}/{name}")), "the new {name} too");
    }

    let scripts = host.scripts();
    let stop = scripts.iter().position(|t| t == "stop").expect("a stop");
    let launch = scripts
        .iter()
        .rposition(|t| t == "launch")
        .expect("a start");
    assert!(stop < launch, "{scripts:?}");
    assert!(
        user.0.lock().unwrap().is_empty(),
        "an update is not a first install"
    );

    // The old image has stopped; the next update clears it away.
    installer(&host, &release, &user).replace_forced().unwrap();
    for aside in &asides {
        assert!(!host.has(aside), "the moved-aside image was swept: {aside}");
    }
    for path in old_resources {
        assert_eq!(
            host.files.lock().unwrap().get(path),
            Some(&(b"old product resource".to_vec(), false)),
            "{path}"
        );
    }
}

/// A Windows host an older, bare-executable install left behind has no ConPTY
/// set beside the server: the next client repairs the set and restarts the
/// daemon into it.
#[test]
fn a_bare_executable_install_is_repaired_and_the_daemon_cycled() {
    let mut host = FakeWindows::new("AMD64").installed(BINARY, ours());
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        host = host.without(&format!("{BIN_DIR}/{name}"));
    }
    let host = host.serving(BINARY);
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));

    let report = installer(&host, &release, &user)
        .run()
        .expect("the repair goes through");

    host.assert_nothing_foreign();
    assert!(
        report.installed,
        "a bare .exe is not an install: {report:?}"
    );
    assert!(report.launched, "the stale daemon was restarted");
    assert!(
        user.0.lock().unwrap().is_empty(),
        "a machine that already has a server is not asked again"
    );
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        assert_eq!(
            host.files
                .lock()
                .unwrap()
                .get(&format!("{BIN_DIR}/{name}"))
                .map(|(bytes, _)| bytes.as_slice()),
            Some(companion_bytes(name)),
            "{name} was restored"
        );
    }
    let scripts = host.scripts();
    let stop = scripts.iter().position(|t| t == "stop").expect("a stop");
    let launch = scripts
        .iter()
        .rposition(|t| t == "launch")
        .expect("a start");
    assert!(stop < launch, "{scripts:?}");
}

/// One missing companion is enough to count the install as broken.
#[test]
fn a_missing_companion_is_restored() {
    let host = FakeWindows::new("AMD64")
        .installed(BINARY, ours())
        .without(&format!("{BIN_DIR}/OpenConsole.exe"))
        .serving(BINARY);
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));

    let report = installer(&host, &release, &user).run().unwrap();

    host.assert_nothing_foreign();
    assert!(report.installed, "an incomplete set is repaired");
    assert_eq!(
        host.files
            .lock()
            .unwrap()
            .get(&format!("{BIN_DIR}/OpenConsole.exe"))
            .map(|(bytes, _)| bytes.as_slice()),
        Some(OPENCONSOLE_BYTES)
    );
}

/// A Windows package that is not the fixed set of files is refused on the
/// client: the user is never asked about it, and nothing is written.
#[test]
fn a_package_missing_a_file_is_refused_before_anything_is_written() {
    let zip = zip_bytes(&[
        (asset::WINDOWS_PACKAGE_EXE, SERVER_BYTES),
        ("conpty.dll", CONPTY_DLL_BYTES),
        ("LICENSE-ConPTY.txt", CONPTY_LICENSE_BYTES),
        ("LICENSE.txt", LICENSE_BYTES),
    ]);
    let host = FakeWindows::new("AMD64");
    let release = Release::with_zip(zip);
    let user = Approve(Mutex::new(Vec::new()));

    let err = installer(&host, &release, &user).run().unwrap_err();

    let InstallError::Package { asset, reason } = &err else {
        panic!("expected a package refusal, got {err:?}");
    };
    assert_eq!(*asset, ASSET_WINDOWS_X86_64);
    assert!(reason.contains("OpenConsole.exe"), "{reason}");
    assert!(
        user.0.lock().unwrap().is_empty(),
        "never asked about a package that is not the release's"
    );
    assert!(!host.has(BIN_DIR), "nothing was written");
    let steps = host.steps();
    assert!(
        !steps
            .iter()
            .any(|s| matches!(s, Step::Put(_) | Step::Rename { .. })),
        "{steps:?}"
    );
}

/// The checksum over the archive cannot say where its paths point: an entry
/// that would escape the install directory is refused with it.
#[test]
fn a_package_with_an_escaping_path_is_refused() {
    let zip = zip_bytes(&[
        (asset::WINDOWS_PACKAGE_EXE, SERVER_BYTES),
        ("conpty.dll", CONPTY_DLL_BYTES),
        ("OpenConsole.exe", OPENCONSOLE_BYTES),
        ("LICENSE-ConPTY.txt", CONPTY_LICENSE_BYTES),
        ("LICENSE.txt", LICENSE_BYTES),
        ("..\\evil.exe", b"MZ not from the release"),
    ]);
    let host = FakeWindows::new("AMD64");
    let release = Release::with_zip(zip);
    let user = Approve(Mutex::new(Vec::new()));

    let err = installer(&host, &release, &user).run().unwrap_err();

    let InstallError::Package { reason, .. } = &err else {
        panic!("expected a package refusal, got {err:?}");
    };
    assert!(reason.contains("flat file name"), "{reason}");
    assert!(!host.has(BIN_DIR), "nothing was written");
}

/// A package whose server speaks another dialect never gets renamed into
/// place — every temp goes back away.
#[test]
fn a_server_speaking_another_dialect_leaves_no_files_behind() {
    let host = FakeWindows::new("AMD64").uploads_speaking(RemoteProtocol {
        control: 99,
        protocol: 99,
        build: VERSION.into(),
    });
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));

    let err = installer(&host, &release, &user).run().unwrap_err();

    assert!(
        matches!(err, InstallError::DialectMismatch { .. }),
        "{err:?}"
    );
    host.assert_nothing_foreign();
    let steps = host.steps();
    let puts = steps.iter().filter(|s| matches!(s, Step::Put(_))).count();
    assert_eq!(puts, 5, "the whole package was tried: {steps:?}");
    let removed = steps
        .iter()
        .filter(|s| matches!(s, Step::Remove(_)))
        .count();
    assert_eq!(removed, 5, "and every temp went back away: {steps:?}");
    assert!(!host.has(BINARY));
    for name in asset::WINDOWS_PACKAGE_COMPANIONS {
        assert!(!host.has(&format!("{BIN_DIR}/{name}")), "{name} not staged");
    }
}

#[test]
fn a_restart_on_windows_asks_the_server_to_stop_instead_of_signalling_it() {
    let host = FakeWindows::new("AMD64")
        .installed(BINARY, ours())
        .serving(BINARY);
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    installer(&host, &release, &user).restart_daemon().unwrap();

    host.assert_nothing_foreign();
    let steps = host.steps();
    assert!(
        !steps.contains(&Step::Uname),
        "a restart knows the platform from the SFTP home: {steps:?}"
    );
    let scripts = host.scripts();
    assert!(
        scripts.windows(2).any(|w| w == ["stop", "control-probe"]),
        "{scripts:?}"
    );
    assert!(scripts.contains(&"launch".to_string()), "{scripts:?}");
}

#[test]
fn another_builds_daemon_is_reported_in_the_installers_spelling() {
    let other = "/C:/Users/me/AppData/Local/lyrel/bin/lyrel-server-c2p4.exe";
    let host = FakeWindows::new("AMD64")
        .installed(BINARY, ours())
        .serving(other);
    host.speaks.lock().unwrap().insert(
        other.to_string(),
        RemoteProtocol {
            control: 2,
            protocol: 4,
            build: "26.8.1".into(),
        },
    );
    let release = Release::new();
    let user = Approve(Mutex::new(Vec::new()));
    let sink = Arc::new(Mutex::new(Vec::new()));
    let report =
        with_mismatch_sink(sink.clone(), || installer(&host, &release, &user).run()).unwrap();

    let mismatch = report.mismatch.expect("the other build is reported");
    assert_eq!(mismatch.running_exe.as_deref(), Some(other));
    assert_eq!(mismatch.running_version.as_deref(), Some("26.8.1"));
    assert_eq!(sink.lock().unwrap().len(), 1);
    host.assert_nothing_foreign();
}

#[test]
fn the_link_command_is_phrased_for_the_hosts_shell() {
    assert_eq!(
        server_stdio_command(BINARY),
        r#""C:\Users\me\AppData\Local\lyrel\bin\lyrel-server-c3p4.exe" --stdio"#
    );
    assert_eq!(
        server_stdio_command("/home/me/.local/share/lyrel/bin/lyrel-server-c3p4"),
        "'/home/me/.local/share/lyrel/bin/lyrel-server-c3p4' --stdio",
        "the unix command is unchanged"
    );
}
