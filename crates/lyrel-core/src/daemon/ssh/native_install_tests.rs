use super::*;
use crate::daemon::install::RemoteOps;
use crate::daemon::install::ssh_ops::SshRemoteOps;
use crate::daemon::install::{
    InstallConfirm, InstallDecision, InstallError, InstallRequest, Installer, LoadedBinary,
    ServerBinarySource,
};
use crate::daemon::protocol::NativeSshSpec;
use crate::daemon::ssh::{
    ConnectionKey, broker::PromptBroker, forward::RemoteForwardTable, handler::ClientHandler,
};

struct Source(Vec<u8>);
impl ServerBinarySource for Source {
    fn load(&self, _version: &str, _asset: &'static str) -> Result<LoadedBinary, InstallError> {
        Ok(LoadedBinary {
            bytes: self.0.clone(),
            origin: "isolated native SSH fixture".into(),
        })
    }
}
struct Approve;
impl InstallConfirm for Approve {
    fn confirm(&self, _request: &InstallRequest) -> InstallDecision {
        InstallDecision::Approve
    }
}

#[test]
#[ignore = "requires an explicitly isolated loopback sshd, fixture key/pin/home and actual server"]
fn native_fish_ssh_installs_restarts_and_reports_refusal() {
    let required = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name} for the isolated fixture"))
    };
    let home = required("LYREL_NATIVE_INSTALL_HOME");
    let key_path = required("LYREL_NATIVE_INSTALL_KEY");
    let known_hosts = required("LYREL_NATIVE_INSTALL_KNOWN_HOSTS");
    let user = required("LYREL_NATIVE_INSTALL_USER");
    let port: u16 = required("LYREL_NATIVE_INSTALL_PORT").parse().unwrap();
    assert!(
        port >= 1024,
        "fixture must use an unprivileged loopback port"
    );
    assert!(
        home.contains("lyrel-175"),
        "fixture home must be explicitly isolated"
    );
    let spec: NativeSshSpec = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1", "port": port, "user": user, "auth_mode": "auto",
        "identity_files": [key_path], "verify_host_keys": true
    }))
    .unwrap();
    let conn = SshManager::global().handle().block_on(async {
        let forwards = RemoteForwardTable::default();
        let handler = ClientHandler {
            host: spec.host.clone(),
            port,
            verify_host_keys: true,
            skip_banner: true,
            known_hosts_path: Some(known_hosts.into()),
            broker: PromptBroker::new(Box::new(|_| false)),
            remote_forwards: forwards.clone(),
        };
        let mut handle = russh::client::connect(
            Arc::new(russh::client::Config::default()),
            ("127.0.0.1", port),
            handler,
        )
        .await
        .unwrap();
        let key = russh::keys::decode_secret_key(
            &std::fs::read_to_string(&spec.identity_files[0]).unwrap(),
            None,
        )
        .unwrap();
        let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
        assert!(
            handle
                .authenticate_publickey(&spec.user, key)
                .await
                .unwrap()
                .success()
        );
        SshConnection::new(handle, ConnectionKey::from_spec(&spec), forwards)
    });
    let ops = SshRemoteOps::new(conn);
    assert_eq!(
        ops.home_dir().unwrap(),
        home,
        "SFTP and the shell must share the fixture home"
    );
    // The production stop selects daemons across dialects. Refuse to exercise
    // it if any matching executable is outside this dedicated fixture home.
    let safety = format!(
        "sh -c {}",
        crate::daemon::install::shell_quote(
            r#"for p in /proc/[0-9]*; do e=$(readlink "$p/exe" 2>/dev/null) || continue; case "$e" in */lyrel-server-*) printf '%s\n' "$e";; esac; done"#
        )
    );
    let processes = ops.run(&safety).unwrap();
    assert!(processes.success());
    for exe in processes.stdout.lines() {
        assert!(
            exe.starts_with(&format!("{home}/")),
            "refusing to touch non-fixture process {exe}"
        );
    }
    let source = Source(std::fs::read(required("LYREL_TEST_SERVER")).unwrap());
    let installer = Installer::with_source(&ops, &source, &Approve, "isolated-native-fish")
        .with_timeouts(Duration::from_secs(5), Duration::from_millis(50));
    installer
        .run()
        .expect("native fish SSH initial/reuse install journey");
    installer
        .replace_forced()
        .expect("native fish SSH upload, protocol verification, launch, daemon probe and restart");
    assert!(
        installer.quick_prove().is_some(),
        "nested POSIX script survives the fish SSH boundary"
    );
    installer.restart_daemon().expect("native fish SSH restart");
    assert!(
        !installer.run().unwrap().launched,
        "native reconnect reuses the restarted daemon"
    );
    let refusal = ops
        .spawn_detached("sh -c 'printf \"fixture shell refusal\\n\" >&2; exit 37'")
        .expect_err("actual shell refusal must not be accepted");
    assert!(
        refusal.contains("37") && refusal.contains("fixture shell refusal"),
        "{refusal}"
    );
}
