//! Real loopback handshakes for the host-key trust policy in
//! [`ClientHandler::check_server_key`].
//!
//! `handler::tests` covers the policy table itself; this module covers the
//! carrying-out: a live `russh` server and client perform the full key
//! exchange on a temporary port, and the assertions land on what the broker
//! was asked and what ended up in a scratch `known_hosts`. No test calls
//! `check_server_key` directly — that would verify the function, not the
//! handshake that is supposed to run it.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::client;
use russh::keys::ssh_key::certificate::{self, CertType};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::public::RsaPublicKey;
use russh::keys::ssh_key::{self, Mpint};
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth};
use tokio::task::JoinHandle;

use crate::daemon::protocol::{AuthPromptKind, AuthResponse, DaemonMsg};
use crate::daemon::ssh::broker::PromptBroker;
use crate::daemon::ssh::forward::RemoteForwardTable;
use crate::daemon::ssh::known_hosts::{self, HostKeyStatus};

use super::ClientHandler;

/// Upper bound for one connect: a scripted answer is instant, so anything
/// close to this means the handshake is stuck rather than slow.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A deterministic host key. Distinct seeds name distinct identities: which
/// of them a test trusts, revokes or rotates is what the assertions key on.
fn host_key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

/// A broker that answers each prompt with the next canned response and
/// records every prompt the handler emitted.
///
/// It answers from inside `emit`, the same way `declining_broker` does:
/// [`PromptBroker::prompt`] files the waiting sender before it emits, so the
/// reply lands on a channel that already exists and neither the two-minute
/// prompt timeout nor the delivery-retry window is ever exercised. A scripted
/// queue that runs dry cancels, which is also the cancel test's answer.
fn scripted_broker(
    answers: Vec<AuthResponse>,
) -> (Arc<PromptBroker>, Arc<Mutex<Vec<AuthPromptKind>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let queue = Mutex::new(VecDeque::from(answers));
    let back: Arc<OnceLock<Weak<PromptBroker>>> = Arc::new(OnceLock::new());
    let emit_back = Arc::clone(&back);
    let asked_emit = Arc::clone(&asked);
    let broker = PromptBroker::new(Box::new(move |msg: DaemonMsg| {
        let DaemonMsg::AuthPrompt { request_id, prompt } = msg else {
            return true;
        };
        asked_emit.lock().unwrap().push(prompt.clone());
        if matches!(prompt, AuthPromptKind::Banner { .. }) {
            // Banners are emitted with request_id 0 and never waited on.
            return true;
        }
        let response = queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AuthResponse::Cancelled);
        if let Some(broker) = emit_back.get().and_then(Weak::upgrade) {
            broker.deliver(request_id, response);
        }
        true
    }));
    let _ = back.set(Arc::downgrade(&broker));
    (broker, asked)
}

/// The server side of every handshake here: it exists only so that a real key
/// exchange and a real `none` authentication happen; the trust decision under
/// test is entirely the client's.
struct TrustSshd;

impl server::Handler for TrustSshd {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }
}

/// A loopback SSH server whose tasks are all tracked and aborted on drop.
struct TestSshd {
    addr: SocketAddr,
    accept: JoinHandle<()>,
    sessions: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Server-side session errors, recorded only so a failed client assertion
    /// can name what the far end saw.
    errors: Arc<Mutex<Vec<String>>>,
}

impl TestSshd {
    async fn start(keys: Vec<PrivateKey>) -> TestSshd {
        Self::start_with(keys, Vec::new()).await
    }

    async fn start_with(
        keys: Vec<PrivateKey>,
        certificates: Vec<ssh_key::Certificate>,
    ) -> TestSshd {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("bound address");
        let mut config = server::Config::default();
        config.inactivity_timeout = None;
        config.keys = keys;
        config.certificates = certificates;
        let sessions: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let accept = tokio::spawn({
            let sessions = Arc::clone(&sessions);
            let errors = Arc::clone(&errors);
            let config = Arc::new(config);
            async move {
                loop {
                    let Ok((socket, _)) = listener.accept().await else {
                        return;
                    };
                    let config = Arc::clone(&config);
                    let errors = Arc::clone(&errors);
                    sessions.lock().unwrap().push(tokio::spawn(async move {
                        let result = match server::run_stream(config, socket, TrustSshd).await {
                            Ok(running) => running.await,
                            Err(e) => Err(e),
                        };
                        if let Err(e) = result {
                            errors.lock().unwrap().push(format!("{e:?}"));
                        }
                    }));
                }
            }
        });
        TestSshd {
            addr,
            accept,
            sessions,
            errors,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for TestSshd {
    fn drop(&mut self) {
        self.accept.abort();
        for session in self.sessions.lock().unwrap().drain(..) {
            session.abort();
        }
    }
}

/// Drive a real connection against `sshd` with a scripted broker and a scratch
/// `known_hosts`. Returns the error when the handshake did not complete.
async fn handshake(
    sshd: &TestSshd,
    verify_host_keys: bool,
    known_hosts_path: std::path::PathBuf,
    answers: Vec<AuthResponse>,
) -> (
    Result<client::Handle<ClientHandler>, russh::Error>,
    Arc<Mutex<Vec<AuthPromptKind>>>,
) {
    handshake_with(
        sshd,
        Arc::new(client::Config::default()),
        verify_host_keys,
        known_hosts_path,
        answers,
    )
    .await
}

/// `handshake` with an explicit client config, for the one test that has to
/// advertise something production never does (host-key certificates).
async fn handshake_with(
    sshd: &TestSshd,
    config: Arc<client::Config>,
    verify_host_keys: bool,
    known_hosts_path: std::path::PathBuf,
    answers: Vec<AuthResponse>,
) -> (
    Result<client::Handle<ClientHandler>, russh::Error>,
    Arc<Mutex<Vec<AuthPromptKind>>>,
) {
    let (broker, asked) = scripted_broker(answers);
    let handler = ClientHandler {
        host: "127.0.0.1".to_string(),
        port: sshd.port(),
        verify_host_keys,
        skip_banner: true,
        known_hosts_path: Some(known_hosts_path),
        broker,
        remote_forwards: RemoteForwardTable::default(),
    };
    let outcome =
        tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, sshd.addr, handler)).await;
    match outcome {
        Ok(result) => (result, asked),
        Err(_) => panic!("the handshake did not finish within {CONNECT_TIMEOUT:?}"),
    }
}

/// A completed handshake proven by a `none` authentication round trip: key
/// exchange alone would pass this point only after `check_server_key` said
/// yes, and this confirms the connection stayed usable after it.
async fn expect_connected(
    result: Result<client::Handle<ClientHandler>, russh::Error>,
    sshd: &TestSshd,
) {
    match result {
        Ok(mut handle) => {
            let auth = tokio::time::timeout(CONNECT_TIMEOUT, handle.authenticate_none("tester"))
                .await
                .expect("authentication timed out")
                .expect("none authentication should succeed");
            assert!(auth.success(), "the fake accepts everyone");
        }
        Err(e) => panic!(
            "expected a completed handshake, got {e:?}; server errors: {:?}",
            sshd.errors.lock().unwrap()
        ),
    }
}

fn expect_rejected(result: Result<client::Handle<ClientHandler>, russh::Error>) {
    let Err(e) = result else {
        panic!("a rejected host key must not complete the handshake");
    };
    assert!(
        matches!(e, russh::Error::UnknownKey),
        "a refused host key surfaces as UnknownKey, got {e:?}"
    );
}

/// The single entry OpenSSH would write: `token`, then the key's algorithm
/// name and base64 blob.
fn known_hosts_line(host: &str, port: u16, key: &PublicKey) -> String {
    let key_openssh = key.to_openssh().expect("a public key encodes");
    format!("{} {key_openssh}\n", known_hosts::host_token(host, port))
}

fn seed_known_hosts(path: &Path, host: &str, port: u16, key: &PublicKey) {
    known_hosts::append_trusted_to(path, host, port, key).expect("seed a known_hosts entry");
}

/// An `ssh-rsa` public key for known_hosts lines. Synthesized — a modulus of
/// fixed bytes and e=65537 — because the only use for it is identity: the
/// host is known under a different *algorithm* than the ed25519 key the test
/// server presents, and no RSA private key exists to stand a server up with.
fn other_algorithm_key() -> PublicKey {
    let mut modulus = [0u8; 64];
    modulus[0] = 0xAA;
    modulus[63] = 0xAB;
    let rsa = RsaPublicKey::new(
        Mpint::from_positive_bytes(&65537u32.to_be_bytes()),
        Mpint::from_positive_bytes(&modulus),
    )
    .expect("a synthesized RSA public key is well-formed");
    PublicKey::from(rsa)
}

/// A host certificate for `subject`, signed by `issuer`. Fixed nonce and
/// serial — nothing about the trust decision is randomized, so neither is
/// the certificate.
fn host_certificate(subject: &PrivateKey, issuer: &PrivateKey) -> ssh_key::Certificate {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs();
    let mut builder = certificate::Builder::new(
        vec![42u8; certificate::Builder::RECOMMENDED_NONCE_SIZE],
        subject.public_key().key_data().clone(),
        now.saturating_sub(3600),
        now + 86400,
    )
    .expect("a certificate validity window");
    builder
        .cert_type(CertType::Host)
        .expect("set the host cert type");
    builder
        .valid_principal("127.0.0.1")
        .expect("set a valid principal");
    builder.sign(issuer).expect("sign the certificate")
}

#[tokio::test]
async fn a_key_already_on_file_is_accepted_silently() {
    let key = host_key(11);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    seed_known_hosts(&store, "127.0.0.1", sshd.port(), key.public_key());
    let before = std::fs::read_to_string(&store).expect("seeded file");

    let (result, asked) = handshake(&sshd, true, store.clone(), Vec::new()).await;
    expect_connected(result, &sshd).await;

    assert!(
        asked.lock().unwrap().is_empty(),
        "a known key must not ask anything, got {:?}",
        asked.lock().unwrap()
    );
    let after = std::fs::read_to_string(&store).expect("known_hosts after");
    assert_eq!(before, after, "a known key must not rewrite the file");
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), key.public_key()),
        HostKeyStatus::Known
    );
}

#[tokio::test]
async fn an_unknown_key_accepted_once_connects_without_recording() {
    let key = host_key(21);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");

    let answers = vec![AuthResponse::HostKeyDecision {
        accept: true,
        remember: false,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_connected(result, &sshd).await;

    let asked = asked.lock().unwrap();
    let [
        AuthPromptKind::HostKeyUnknown {
            host,
            port,
            algorithm,
            fingerprint_sha256,
            previously_known_as,
        },
    ] = asked.as_slice()
    else {
        panic!(
            "an unknown key asks exactly once, got {:?}",
            asked.as_slice()
        );
    };
    assert_eq!(host, "127.0.0.1");
    assert_eq!(*port, sshd.port());
    assert_eq!(algorithm, "ssh-ed25519");
    assert_eq!(
        fingerprint_sha256,
        &known_hosts::fingerprint_sha256(key.public_key())
    );
    assert_eq!(previously_known_as, &None);
    drop(asked);

    assert!(
        !store.exists(),
        "accepting without remember must not create known_hosts"
    );
}

#[tokio::test]
async fn an_unknown_key_accepted_and_remembered_is_trusted_next_time() {
    let key = host_key(31);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");

    let answers = vec![AuthResponse::HostKeyDecision {
        accept: true,
        remember: true,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_connected(result, &sshd).await;
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "one unknown key, one prompt"
    );

    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), key.public_key()),
        HostKeyStatus::Known,
        "accept-and-remember persists the key"
    );

    // The next connection answers Known from the file alone — no prompt, and
    // a write path that ran would show up as a second append.
    let (result, asked2) = handshake(&sshd, true, store.clone(), Vec::new()).await;
    expect_connected(result, &sshd).await;
    assert!(
        asked2.lock().unwrap().is_empty(),
        "a remembered key must not be asked about again"
    );
    let contents = std::fs::read_to_string(&store).expect("known_hosts after");
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one key was recorded");
}

#[tokio::test]
async fn a_rejected_key_never_records_even_with_remember_ticked() {
    let key = host_key(41);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");

    // `remember: true` alongside `accept: false` is the dialog's checkbox
    // state, not a request to trust the key.
    let answers = vec![AuthResponse::HostKeyDecision {
        accept: false,
        remember: true,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_rejected(result);

    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(
        !store.exists(),
        "a rejection must not create a known_hosts entry"
    );
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), key.public_key()),
        HostKeyStatus::Unknown,
        "the key stays untrusted"
    );
}

#[tokio::test]
async fn a_cancelled_prompt_rejects_without_recording() {
    let key = host_key(51);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");

    let answers = vec![AuthResponse::Cancelled];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_rejected(result);

    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(
        !store.exists(),
        "cancelling must not create a known_hosts entry"
    );
}

#[tokio::test]
async fn an_accepted_replacement_replaces_the_key_it_contradicts() {
    let old_key = host_key(61);
    let new_key = host_key(62);
    let sshd = TestSshd::start(vec![new_key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    seed_known_hosts(&store, "127.0.0.1", sshd.port(), old_key.public_key());

    let answers = vec![AuthResponse::HostKeyDecision {
        accept: true,
        remember: true,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_connected(result, &sshd).await;

    let asked = asked.lock().unwrap();
    let [
        AuthPromptKind::HostKeyChanged {
            algorithm,
            fingerprint_sha256,
            old_fingerprint_sha256,
            ..
        },
    ] = asked.as_slice()
    else {
        panic!(
            "a contradicting key uses the changed-key prompt, got {:?}",
            asked.as_slice()
        );
    };
    assert_eq!(algorithm, "ssh-ed25519");
    assert_eq!(
        fingerprint_sha256,
        &known_hosts::fingerprint_sha256(new_key.public_key())
    );
    assert_eq!(
        old_fingerprint_sha256,
        &known_hosts::fingerprint_sha256(old_key.public_key())
    );
    drop(asked);

    // The line the new key contradicts is gone: check_in_file answers Known
    // on any same-algorithm match, so an append alone would have left the
    // replaced key trusted forever.
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), old_key.public_key()),
        HostKeyStatus::Changed {
            old_fingerprint_sha256: known_hosts::fingerprint_sha256(new_key.public_key()),
        },
        "the superseded key must read as contradicting the recorded one"
    );
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), new_key.public_key()),
        HostKeyStatus::Known
    );

    // And a fresh handshake treats it as the known host it now is.
    let (result, asked2) = handshake(&sshd, true, store.clone(), Vec::new()).await;
    expect_connected(result, &sshd).await;
    assert!(asked2.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_key_under_a_new_algorithm_asks_the_unknown_prompt_and_keeps_the_old_line() {
    let old_rsa = other_algorithm_key();
    let new_key = host_key(71);
    let sshd = TestSshd::start(vec![new_key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    seed_known_hosts(&store, "127.0.0.1", sshd.port(), &old_rsa);

    let answers = vec![AuthResponse::HostKeyDecision {
        accept: true,
        remember: true,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_connected(result, &sshd).await;

    let asked = asked.lock().unwrap();
    // A key new only because its *algorithm* is new is not the
    // man-in-the-middle warning; it is the unknown-host prompt that names
    // what the host was previously known by.
    let [
        AuthPromptKind::HostKeyUnknown {
            previously_known_as,
            ..
        },
    ] = asked.as_slice()
    else {
        panic!(
            "a new-algorithm key uses the unknown-host prompt, got {:?}",
            asked.as_slice()
        );
    };
    assert_eq!(previously_known_as.as_deref(), Some("ssh-rsa"));
    drop(asked);

    // The replacement rule only retires a same-algorithm contradiction: the
    // ssh-rsa entry is uncontradicted and stays, exactly as OpenSSH keeps it.
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), &old_rsa),
        HostKeyStatus::Known,
        "the other-algorithm entry is not superseded"
    );
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), new_key.public_key()),
        HostKeyStatus::Known
    );
    let contents = std::fs::read_to_string(&store).expect("known_hosts after");
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "both the old ssh-rsa line and the new ssh-ed25519 line are on file"
    );
}

#[tokio::test]
async fn a_key_under_a_new_algorithm_rejected_writes_nothing() {
    let old_rsa = other_algorithm_key();
    let new_key = host_key(81);
    let sshd = TestSshd::start(vec![new_key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    seed_known_hosts(&store, "127.0.0.1", sshd.port(), &old_rsa);
    let before = std::fs::read_to_string(&store).expect("seeded file");

    let answers = vec![AuthResponse::HostKeyDecision {
        accept: false,
        remember: true,
    }];
    let (result, asked) = handshake(&sshd, true, store.clone(), answers).await;
    expect_rejected(result);

    assert_eq!(asked.lock().unwrap().len(), 1);
    let after = std::fs::read_to_string(&store).expect("known_hosts after");
    assert_eq!(before, after, "rejecting must not touch the file");
    assert_eq!(
        known_hosts::check_in_file(&store, "127.0.0.1", sshd.port(), &old_rsa),
        HostKeyStatus::Known,
        "the pre-existing entry survives a refusal"
    );
}

/// `@revoked` is a standing refusal, not a trust record: it rejects under
/// `verify_host_keys = true`, and — the arm the whole feature exists for —
/// still rejects with verification switched off, where every other status
/// would sail through.
#[tokio::test]
async fn a_revoked_key_is_rejected_under_either_verification_setting() {
    let key = host_key(91);
    let sshd = TestSshd::start(vec![key.clone()]).await;
    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    let revoked_line = format!(
        "@revoked {}",
        known_hosts_line("127.0.0.1", sshd.port(), key.public_key())
    );
    std::fs::write(&store, &revoked_line).expect("seed a revoked entry");
    let before = std::fs::read_to_string(&store).expect("seeded file");

    for verify_host_keys in [true, false] {
        let (result, asked) = handshake(&sshd, verify_host_keys, store.clone(), Vec::new()).await;
        expect_rejected(result);
        assert!(
            asked.lock().unwrap().is_empty(),
            "a revoked key is refused outright with verify_host_keys={verify_host_keys}, got {:?}",
            asked.lock().unwrap()
        );
    }
    let after = std::fs::read_to_string(&store).expect("known_hosts after");
    assert_eq!(before, after, "a revoked key never rewrites the file");
}

/// A negotiated host certificate must not inherit trust from its bare key.
/// Explicitly offer the certificate algorithm to exercise the refusal through
/// a real key exchange; the production preference list does not offer it.
#[tokio::test]
async fn a_host_key_certificate_is_rejected_though_its_inner_key_is_known() {
    let subject = host_key(101);
    let issuer = host_key(102);
    let cert = host_certificate(&subject, &issuer);
    let sshd = TestSshd::start_with(vec![subject.clone()], vec![cert]).await;

    let dir = tempfile::tempdir().expect("scratch dir");
    let store = dir.path().join("known_hosts");
    // Even with the certificate's bare subject key trusted outright, the
    // handshake refuses: a certificate is not the thing on file.
    seed_known_hosts(&store, "127.0.0.1", sshd.port(), subject.public_key());
    let before = std::fs::read_to_string(&store).expect("seeded file");

    let mut config = client::Config::default();
    config.preferred.host_key_certificates =
        std::borrow::Cow::Borrowed(&[russh::keys::Algorithm::Ed25519][..]);
    let (result, asked) =
        handshake_with(&sshd, Arc::new(config), true, store.clone(), Vec::new()).await;
    expect_rejected(result);

    assert!(
        asked.lock().unwrap().is_empty(),
        "a certificate is refused without consulting anyone, got {:?}",
        asked.lock().unwrap()
    );
    let after = std::fs::read_to_string(&store).expect("known_hosts after");
    assert_eq!(before, after);
}
