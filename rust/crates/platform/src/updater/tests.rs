use std::sync::atomic::{AtomicUsize, Ordering};

use digest::Digest;
use ed25519_dalek::Signer;
use sha2::{Sha256, Sha512};

use super::extract::tests::{Entry, zip_bytes};
use super::*;

#[derive(Default)]
struct FakeHost {
    events: Mutex<Vec<UpdaterEvent>>,
    quits: AtomicUsize,
}

impl Host for FakeHost {
    fn emit(&self, event: UpdaterEvent) {
        self.events.lock().push(event);
    }

    fn quit(&self) {
        self.quits.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakeHost {
    fn names(&self) -> Vec<&'static str> {
        self.events.lock().iter().map(UpdaterEvent::name).collect()
    }

    fn error_info(&self) -> Option<ErrorInfo> {
        self.events.lock().iter().find_map(|e| match e {
            UpdaterEvent::Error(info) => Some(info.clone()),
            _ => None,
        })
    }
}

#[derive(Default)]
struct FakeProvider {
    name: &'static str,
    rel: Option<Release>,
    check_err: Option<&'static str>,
    body: Vec<u8>,
    calls: AtomicUsize,
    downloads: AtomicUsize,
}

impl Provider for FakeProvider {
    fn name(&self) -> &str {
        self.name
    }

    fn check<'a>(
        &'a self,
        _req: &'a CheckRequest,
    ) -> BoxFuture<'a, Result<Option<Release>, ProviderError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match self.check_err {
                Some(e) => Err(e.into()),
                None => Ok(self.rel.clone()),
            }
        })
    }

    fn download<'a>(
        &'a self,
        _rel: &'a Release,
        dst: &'a mut (dyn Write + Send),
        on_progress: &'a mut (dyn FnMut(i64, i64) + Send),
    ) -> BoxFuture<'a, Result<(), ProviderError>> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            dst.write_all(&self.body)?;
            on_progress(self.body.len() as i64, self.body.len() as i64);
            Ok(())
        })
    }
}

fn release(filename: &str, body: &[u8], verification: Option<Verification>) -> Release {
    Release {
        version: "2.0.0".into(),
        artifact: Artifact {
            filename: filename.into(),
            size: body.len() as i64,
            ..Artifact::default()
        },
        verification,
        ..Release::default()
    }
}

fn provider(name: &'static str, rel: Option<Release>, body: &[u8]) -> Arc<FakeProvider> {
    Arc::new(FakeProvider {
        name,
        rel,
        body: body.to_vec(),
        ..FakeProvider::default()
    })
}

fn config(providers: Vec<Arc<dyn Provider>>, public_key: Vec<u8>) -> Config {
    Config {
        current_version: "1.0.0".into(),
        providers,
        public_key,
        helper_ready_timeout: Duration::ZERO,
        platform: String::new(),
        arch: String::new(),
    }
}

/// Tests that stage downloads hold this shared; the leak check holds it
/// exclusively so concurrent tests do not skew its directory count.
static STAGING: parking_lot::RwLock<()> = parking_lot::RwLock::new(());

/// Removes the staged `wails-update-*` dir when the test ends.
struct Staged(
    Arc<Updater>,
    #[expect(dead_code, reason = "held for the test\'s lifetime")]
    Option<parking_lot::RwLockReadGuard<'static, ()>>,
);

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(p) = self.0.downloaded_path() {
            let dir = p.ancestors().find(|d| {
                d.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("wails-update-"))
            });
            if let Some(dir) = dir {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

fn configured(host: &Arc<FakeHost>, providers: Vec<Arc<dyn Provider>>, key: Vec<u8>) -> Staged {
    let guard = STAGING.read();
    let u = Updater::new(Arc::clone(host) as Arc<dyn Host>);
    u.init(config(providers, key)).expect("init");
    Staged(u, Some(guard))
}

fn compact(names: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for n in names {
        if out.last() == Some(n) && *n == "wails:updater:download-progress" {
            continue;
        }
        out.push(n);
    }
    out
}

fn assert_subsequence(got: &[&str], want: &[&str]) {
    let mut idx = 0;
    for g in got {
        if idx < want.len() && *g == want[idx] {
            idx += 1;
        }
    }
    assert_eq!(
        idx,
        want.len(),
        "got {got:?}, required subsequence {want:?}"
    );
}

#[test]
fn init_validation() {
    let u = Updater::new(Arc::new(FakeHost::default()));
    let p: Arc<dyn Provider> = provider("f", None, b"");
    let mut cfg = config(vec![Arc::clone(&p)], vec![]);
    cfg.current_version.clear();
    assert!(u.init(cfg).is_err(), "missing CurrentVersion");
    assert!(u.init(config(vec![], vec![])).is_err(), "empty providers");
    assert_eq!(u.state(), State::Unconfigured);

    u.init(config(vec![Arc::clone(&p)], vec![])).expect("init");
    assert_eq!(u.state(), State::Idle);
    assert_eq!(
        u.init(config(vec![p], vec![])),
        Err(UpdaterError::AlreadyConfigured)
    );
}

#[tokio::test]
async fn check_not_configured() {
    let u = Updater::new(Arc::new(FakeHost::default()));
    assert_eq!(u.check().await, Err(UpdaterError::NotConfigured));
}

#[tokio::test]
async fn check_update_available() {
    let host = Arc::new(FakeHost::default());
    let p = provider("primary", Some(release("app.dmg", b"12345", None)), b"");
    let u = configured(&host, vec![p], vec![]);
    let got = u.0.check().await.expect("check").expect("release");
    assert_eq!(got.version, "2.0.0");
    assert_eq!(got.provider, "primary");
    assert_eq!(u.0.state(), State::Available);
    assert_eq!(
        compact(&host.names()),
        [
            "wails:updater:check-started",
            "wails:updater:update-available"
        ]
    );
}

#[tokio::test]
async fn up_to_date_stops_fallback_chain() {
    let host = Arc::new(FakeHost::default());
    let primary = provider("primary", None, b"");
    let secondary = provider(
        "secondary",
        Some(Release {
            version: "9.9.9".into(),
            ..Release::default()
        }),
        b"",
    );
    let u = configured(&host, vec![primary, Arc::clone(&secondary) as _], vec![]);
    assert_eq!(u.0.check().await, Ok(None));
    assert_eq!(secondary.calls.load(Ordering::SeqCst), 0);
    assert_eq!(u.0.state(), State::UpToDate);
    assert_eq!(
        compact(&host.names()),
        ["wails:updater:check-started", "wails:updater:no-update"]
    );
}

#[tokio::test]
async fn falls_back_on_error() {
    let host = Arc::new(FakeHost::default());
    let primary = Arc::new(FakeProvider {
        name: "primary",
        check_err: Some("network down"),
        ..FakeProvider::default()
    });
    let secondary = provider("secondary", Some(release("app.dmg", b"", None)), b"");
    let u = configured(&host, vec![primary, secondary], vec![]);
    let got = u.0.check().await.expect("check").expect("release");
    assert_eq!(got.provider, "secondary");
}

#[tokio::test]
async fn all_providers_error() {
    let host = Arc::new(FakeHost::default());
    let p1 = Arc::new(FakeProvider {
        name: "primary",
        check_err: Some("dns"),
        ..FakeProvider::default()
    });
    let p2 = Arc::new(FakeProvider {
        name: "secondary",
        check_err: Some("401"),
        ..FakeProvider::default()
    });
    let u = configured(&host, vec![p1, p2], vec![]);
    let err = u.0.check().await.expect_err("all fail").to_string();
    assert!(
        err.contains("primary") && err.contains("secondary"),
        "{err}"
    );
    assert_eq!(u.0.state(), State::Error);
    assert_eq!(
        compact(&host.names()),
        ["wails:updater:check-started", "wails:updater:error"]
    );
}

#[tokio::test]
async fn skipped_version_reads_as_up_to_date() {
    let host = Arc::new(FakeHost::default());
    let u = configured(
        &host,
        vec![provider("p", Some(release("a", b"", None)), b"")],
        vec![],
    );
    u.0.skip_version("2.0.0");
    assert_eq!(u.0.check().await, Ok(None));
    assert_eq!(u.0.skipped_version(), "2.0.0");
}

#[tokio::test]
async fn download_without_pending_release() {
    let host = Arc::new(FakeHost::default());
    let u = configured(&host, vec![provider("p", None, b"")], vec![]);
    assert_eq!(
        u.0.download_and_install().await,
        Err(UpdaterError::NoPendingRelease)
    );
}

#[tokio::test]
async fn happy_path_without_verification() {
    let host = Arc::new(FakeHost::default());
    let body = b"hello-end-to-end";
    let u = configured(
        &host,
        vec![provider("p", Some(release("app.bin", body, None)), body)],
        vec![],
    );
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    assert_eq!(u.0.state(), State::Ready);
    assert_subsequence(
        &host.names(),
        &[
            "wails:updater:check-started",
            "wails:updater:update-available",
            "wails:updater:download-started",
            "wails:updater:download-progress",
            "wails:updater:download-complete",
            "wails:updater:verifying",
            "wails:updater:installing",
            "wails:updater:update-ready",
        ],
    );
    let staged = u.0.downloaded_path().expect("staged");
    assert_eq!(staged.file_name().expect("name"), "app.bin");
    assert_eq!(std::fs::read(&staged).expect("read"), body);
    let progress = host
        .events
        .lock()
        .iter()
        .rev()
        .find_map(|e| match e {
            UpdaterEvent::DownloadProgress(p) => Some(p.clone()),
            _ => None,
        })
        .expect("progress");
    assert_eq!(
        (progress.written, progress.total),
        (body.len() as i64, body.len() as i64)
    );
    assert_eq!(progress.provider, "p");
}

fn sha256(b: &[u8]) -> Vec<u8> {
    Sha256::digest(b).to_vec()
}

fn digest_only(d: Vec<u8>) -> Option<Verification> {
    Some(Verification {
        digest_algo: "sha256".into(),
        digest: d,
        ..Verification::default()
    })
}

fn count_staging_dirs() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && e.file_name().to_string_lossy().starts_with("wails-update-")
                })
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
#[allow(clippy::await_holding_lock, reason = "test-wide exclusive section")]
async fn digest_mismatch_fails_closed_and_cleans_up() {
    let host = Arc::new(FakeHost::default());
    let body = b"real-bytes";
    let rel = release("app.bin", body, digest_only(sha256(b"different-bytes")));
    let _exclusive = STAGING.write();
    let u = Updater::new(Arc::clone(&host) as Arc<dyn Host>);
    u.init(config(vec![provider("p", Some(rel), body)], vec![]))
        .expect("init");
    let u = Staged(u, None);
    let before = count_staging_dirs();
    u.0.check().await.expect("check");
    let err = u.0.download_and_install().await.expect_err("mismatch");
    assert!(err.to_string().contains("digest mismatch"), "{err}");
    assert_eq!(u.0.state(), State::Error);
    assert_eq!(u.0.downloaded_path(), None);
    let info = host.error_info().expect("error event");
    assert_eq!(info.stage, Stage::Verify);
    assert_eq!(info.provider, "p");
    assert!(count_staging_dirs() <= before, "staging dir leaked");
}

#[tokio::test]
async fn digest_match_succeeds() {
    let host = Arc::new(FakeHost::default());
    let body = b"real-bytes";
    let rel = release("app.bin", body, digest_only(sha256(body)));
    let u = configured(&host, vec![provider("p", Some(rel), body)], vec![]);
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    assert_eq!(u.0.state(), State::Ready);
}

#[tokio::test]
async fn zip_bundle_is_extracted() {
    let host = Arc::new(FakeHost::default());
    let body = zip_bytes(&[
        Entry::Dir("MyApp.app/"),
        Entry::Dir("MyApp.app/Contents/"),
        Entry::File("MyApp.app/Contents/Info.plist", b"<plist/>", 0o644),
        Entry::Dir("MyApp.app/Contents/MacOS/"),
        Entry::File("MyApp.app/Contents/MacOS/MyApp", b"ELF", 0o755),
    ]);
    let rel = release("MyApp-darwin.zip", &body, digest_only(sha256(&body)));
    let u = configured(&host, vec![provider("p", Some(rel), &body)], vec![]);
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    let staged = u.0.downloaded_path().expect("staged");
    assert!(staged.is_dir(), "staged path is the extracted bundle");
    assert_eq!(staged.file_name().expect("name"), "MyApp.app");
    assert_eq!(
        std::fs::read(staged.join("Contents/Info.plist")).expect("read"),
        b"<plist/>"
    );
}

fn ed25519_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&rand_bytes::<32>())
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let mut h = Sha512::new();
    h.update(seed.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    h.update(SEQ.fetch_add(1, Ordering::SeqCst).to_le_bytes());
    out.copy_from_slice(&h.finalize()[..N]);
    out
}

fn ed25519_pkix(key: &ed25519_dalek::VerifyingKey) -> Vec<u8> {
    // SubjectPublicKeyInfo { AlgorithmIdentifier { id-Ed25519 }, BIT STRING }.
    let mut der = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    der.extend_from_slice(key.as_bytes());
    der
}

#[tokio::test]
async fn ed25519_signature_verifies() {
    let host = Arc::new(FakeHost::default());
    let key = ed25519_key();
    let body = b"signed-payload";
    let sig = key.sign(&sha256(body)).to_bytes().to_vec();
    let rel = release(
        "app.bin",
        body,
        Some(Verification {
            digest_algo: "sha256".into(),
            signature_algo: "ed25519".into(),
            signature: sig,
            ..Verification::default()
        }),
    );
    let raw_key = key.verifying_key().as_bytes().to_vec();
    let u = configured(&host, vec![provider("p", Some(rel), body)], raw_key);
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    assert_eq!(u.0.state(), State::Ready);
}

/// Ed25519ph (RFC 8032): sign SHA-512(message) with the "SigEd25519 no
/// Ed25519 collisions" dom2 prefix, as Go's `ed25519.Options{Hash: SHA512}`.
pub(super) fn sign_ed25519ph(key: &ed25519_dalek::SigningKey, prehash: &[u8; 64]) -> Vec<u8> {
    #[derive(Clone)]
    struct Pre([u8; 64]);
    impl Default for Pre {
        fn default() -> Self {
            Self([0; 64])
        }
    }
    impl digest::HashMarker for Pre {}
    impl digest::Update for Pre {
        fn update(&mut self, _: &[u8]) {}
    }
    impl digest::OutputSizeUser for Pre {
        type OutputSize = digest::consts::U64;
    }
    impl digest::FixedOutput for Pre {
        fn finalize_into(self, out: &mut digest::Output<Self>) {
            out.copy_from_slice(&self.0);
        }
    }
    key.sign_prehashed(Pre(*prehash), None)
        .expect("sign")
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn ed25519ph_signature_verifies() {
    let host = Arc::new(FakeHost::default());
    let key = ed25519_key();
    let body = b"signed-payload-ph";
    let prehash: [u8; 64] = Sha512::digest(body).into();
    let rel = release(
        "app.bin",
        body,
        Some(Verification {
            digest_algo: "sha512".into(),
            signature_algo: "ed25519ph".into(),
            signature: sign_ed25519ph(&key, &prehash),
            ..Verification::default()
        }),
    );
    let pkix = ed25519_pkix(&key.verifying_key());
    let u = configured(&host, vec![provider("p", Some(rel), body)], pkix);
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
}

fn p256_key() -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_slice(&rand_bytes::<32>()).expect("p256 key")
}

fn p256_pkix(key: &p256::ecdsa::VerifyingKey) -> Vec<u8> {
    // SubjectPublicKeyInfo { { id-ecPublicKey, prime256v1 }, BIT STRING }.
    let point = key.to_sec1_point(false);
    let mut der = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    der.extend_from_slice(point.as_bytes());
    der
}

fn p256_der_sig(key: &p256::ecdsa::SigningKey, digest: &[u8]) -> Vec<u8> {
    use p256::ecdsa::signature::hazmat::PrehashSigner;
    let sig: p256::ecdsa::Signature = key.sign_prehash(digest).expect("sign");
    sig.to_der().as_bytes().to_vec()
}

fn ecdsa_release(body: &[u8], sig: Vec<u8>) -> Release {
    release(
        "app.bin",
        body,
        Some(Verification {
            digest_algo: "sha256".into(),
            signature_algo: "ecdsa-p256".into(),
            signature: sig,
            ..Verification::default()
        }),
    )
}

#[tokio::test]
async fn ecdsa_p256_signature_verifies() {
    let host = Arc::new(FakeHost::default());
    let key = p256_key();
    let body = b"ecdsa-payload";
    let sig = p256_der_sig(&key, &sha256(body));
    let pkix = p256_pkix(key.verifying_key());
    let u = configured(
        &host,
        vec![provider("p", Some(ecdsa_release(body, sig)), body)],
        pkix,
    );
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
}

#[tokio::test]
async fn ecdsa_trailing_bytes_rejected() {
    let host = Arc::new(FakeHost::default());
    let key = p256_key();
    let body = b"ecdsa-payload";
    let mut sig = p256_der_sig(&key, &sha256(body));
    sig.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let pkix = p256_pkix(key.verifying_key());
    let u = configured(
        &host,
        vec![provider("p", Some(ecdsa_release(body, sig)), body)],
        pkix,
    );
    u.0.check().await.expect("check");
    let err = u.0.download_and_install().await.expect_err("trailing");
    assert!(err.to_string().contains("trailing"), "{err}");
}

#[tokio::test]
async fn signature_without_public_key_fails() {
    let host = Arc::new(FakeHost::default());
    let body = b"payload";
    let rel = release(
        "app.bin",
        body,
        Some(Verification {
            digest_algo: "sha256".into(),
            signature_algo: "ed25519".into(),
            signature: b"doesn't matter".to_vec(),
            ..Verification::default()
        }),
    );
    let u = configured(&host, vec![provider("p", Some(rel), body)], vec![]);
    u.0.check().await.expect("check");
    let err = u.0.download_and_install().await.expect_err("no key");
    assert!(err.to_string().contains("requires a public key"), "{err}");
}

#[tokio::test]
async fn signature_rejects_attacker_key() {
    let host = Arc::new(FakeHost::default());
    let attacker = ed25519_key();
    let body = b"payload";
    let sig = attacker.sign(&sha256(body)).to_bytes().to_vec();
    let pinned = ed25519_pkix(&ed25519_key().verifying_key());
    let rel = release(
        "app.bin",
        body,
        Some(Verification {
            digest_algo: "sha256".into(),
            signature_algo: "ed25519".into(),
            signature: sig,
            ..Verification::default()
        }),
    );
    let u = configured(&host, vec![provider("p", Some(rel), body)], pinned);
    u.0.check().await.expect("check");
    let err = u.0.download_and_install().await.expect_err("attacker key");
    assert!(err.to_string().contains("did not verify"), "{err}");
}

#[tokio::test]
async fn check_and_install_up_to_date_is_noop() {
    let host = Arc::new(FakeHost::default());
    let p = provider("p", None, b"");
    let u = configured(&host, vec![Arc::clone(&p) as _], vec![]);
    u.0.check_and_install().await.expect("check_and_install");
    assert_eq!(p.downloads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn check_and_install_happy_path() {
    let host = Arc::new(FakeHost::default());
    let body = b"payload";
    let rel = release("app.bin", body, digest_only(sha256(body)));
    let u = configured(&host, vec![provider("p", Some(rel), body)], vec![]);
    u.0.check_and_install().await.expect("check_and_install");
    assert_eq!(u.0.state(), State::Ready);
}

#[tokio::test]
async fn user_actions_need_a_session_and_replay_state() {
    let host = Arc::new(FakeHost::default());
    let body = b"payload";
    let u = configured(
        &host,
        vec![provider("p", Some(release("app.bin", body, None)), body)],
        vec![],
    );
    u.0.user_action(UserAction::WindowReady);
    assert!(host.names().is_empty(), "no session yet");

    u.0.check_and_install().await.expect("check_and_install");
    host.events.lock().clear();
    u.0.user_action(UserAction::WindowReady);
    assert_eq!(
        host.names(),
        ["wails:updater:meta", "wails:updater:update-ready"]
    );
    match &host.events.lock()[0] {
        UpdaterEvent::Meta(m) => assert_eq!(m.current_version, "1.0.0"),
        other => panic!("{other:?}"),
    }

    u.0.user_action(UserAction::Skip);
    assert_eq!(u.0.skipped_version(), "2.0.0");
    host.events.lock().clear();
    u.0.user_action(UserAction::WindowReady);
    assert!(host.names().is_empty(), "skip closed the session");
}

#[test]
fn event_wire_names_and_payloads() {
    let names = [
        UpdaterEvent::CheckStarted.name(),
        UpdaterEvent::UpdateAvailable(Release::default()).name(),
        UpdaterEvent::NoUpdate.name(),
        UpdaterEvent::Error(ErrorInfo {
            stage: Stage::Check,
            message: String::new(),
            provider: String::new(),
        })
        .name(),
        UpdaterEvent::UpdateReady(Release::default()).name(),
    ];
    assert_eq!(
        names,
        [
            "wails:updater:check-started",
            "wails:updater:update-available",
            "wails:updater:no-update",
            "wails:updater:error",
            "wails:updater:update-ready",
        ]
    );
    assert_eq!(
        UserAction::from_event_name("wails:updater:user:restart"),
        Some(UserAction::Restart)
    );
    assert_eq!(UserAction::from_event_name("updater.check"), None);

    let err = UpdaterEvent::Error(ErrorInfo {
        stage: Stage::Verify,
        message: "boom".into(),
        provider: "p".into(),
    });
    assert_eq!(
        err.payload(),
        serde_json::json!({"stage": "verify", "message": "boom", "provider": "p"})
    );
    let rel = UpdaterEvent::UpdateReady(Release {
        version: "2.0.0".into(),
        verification: digest_only(vec![1, 2]),
        ..Release::default()
    });
    assert_eq!(
        rel.payload(),
        serde_json::json!({
            "version": "2.0.0",
            "publishedAt": "0001-01-01T00:00:00Z",
            "artifact": {"filename": ""},
            "verification": {"digestAlgo": "sha256", "digest": "AQI="},
        })
    );
    assert_eq!(UpdaterEvent::NoUpdate.payload(), serde_json::Value::Null);
}

#[test]
fn bundle_targets() {
    if cfg!(target_os = "macos") {
        for (exe, want) in [
            (
                "/Applications/MyApp.app/Contents/MacOS/MyApp",
                "/Applications/MyApp.app",
            ),
            (
                "/Users/user/Desktop/My App.app/Contents/MacOS/myapp",
                "/Users/user/Desktop/My App.app",
            ),
            ("/usr/local/bin/mytool", "/usr/local/bin/mytool"),
            ("./myapp", "./myapp"),
        ] {
            assert_eq!(bundle_target(Path::new(exe)), Path::new(want), "{exe}");
        }
    } else {
        assert_eq!(
            bundle_target(Path::new("/opt/x/recur")),
            Path::new("/opt/x/recur")
        );
    }
}

#[test]
fn finalise_neutralises_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, want) in [
        ("app.zip", "app.zip"),
        ("../../evil", "evil"),
        ("..", "wails-update.bin"),
        ("", "wails-update.bin"),
        ("a\\b", "wails-update.bin"),
    ] {
        let tmp = dir.path().join(".artifact");
        std::fs::write(&tmp, "x").expect("write");
        let got = finalise_download(&tmp, name).expect("finalise");
        assert_eq!(got, dir.path().join(want), "{name:?}");
        std::fs::remove_file(got).expect("rm");
    }
}

fn restart_updater(
    host: &Arc<FakeHost>,
    body: &[u8],
    hooks: RestartHooks,
    timeout: Duration,
) -> Staged {
    let u = Arc::new(Updater {
        host: Arc::clone(host) as Arc<dyn Host>,
        inner: Mutex::default(),
        download_lock: tokio::sync::Mutex::new(()),
        session: Mutex::new(false),
        hooks,
        rt: None,
    });
    let mut cfg = config(
        vec![provider("p", Some(release("app.bin", body, None)), body)],
        vec![],
    );
    cfg.helper_ready_timeout = timeout;
    u.init(cfg).expect("init");
    Staged(u, Some(STAGING.read()))
}

fn benign_hooks(ready: Result<(), UpdaterError>) -> RestartHooks {
    RestartHooks {
        self_exe: || Ok(PathBuf::from("/usr/bin/true")),
        command: Box::new(|p| std::process::Command::new(p)),
        wait_ready: Box::new(move |_, _| {
            let ready = ready.clone();
            Box::pin(async move { ready })
        }),
    }
}

#[tokio::test]
async fn restart_quits_after_spawn() {
    let host = Arc::new(FakeHost::default());
    let u = restart_updater(&host, b"payload", benign_hooks(Ok(())), Duration::ZERO);
    assert_eq!(u.0.restart().await, Err(UpdaterError::NotReady));
    assert_eq!(
        host.quits.load(Ordering::SeqCst),
        0,
        "quit dispatched too early"
    );
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    u.0.restart().await.expect("restart");
    assert_eq!(host.quits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_does_not_quit_before_helper_ready() {
    let host = Arc::new(FakeHost::default());
    let hooks = benign_hooks(Err(UpdaterError::HelperNotReady(String::new())));
    let u = restart_updater(&host, b"payload", hooks, Duration::ZERO);
    u.0.check().await.expect("check");
    u.0.download_and_install().await.expect("install");
    assert!(matches!(
        u.0.restart().await,
        Err(UpdaterError::HelperNotReady(_))
    ));
    assert_eq!(host.quits.load(Ordering::SeqCst), 0);
}

const HELPER_PID_FILE_ENV: &str = "RECUR_TEST_HELPER_PID_FILE";
const HELPER_DELAY_ENV: &str = "RECUR_TEST_HELPER_DELAY_MS";

/// Re-executed by `restart_helper_startup_deadline`: simulates slow app
/// start-up, then enters the real helper mode.
#[test]
fn restart_helper_process() {
    let Ok(pid_file) = std::env::var(HELPER_PID_FILE_ENV) else {
        return;
    };
    std::fs::write(&pid_file, std::process::id().to_string()).expect("pid file");
    let delay: u64 = std::env::var(HELPER_DELAY_ENV)
        .expect("delay")
        .parse()
        .expect("delay ms");
    std::thread::sleep(Duration::from_millis(delay));
    handle_helper_mode();
    panic!("helper mode unexpectedly returned");
}

#[tokio::test]
async fn restart_helper_startup_deadline() {
    for (name, delay_ms, timeout, want_err) in [
        (
            "default permits slow initialization",
            6_000,
            Duration::ZERO,
            false,
        ),
        (
            "configured timeout permits startup",
            150,
            Duration::from_secs(2),
            false,
        ),
        (
            "configured timeout stops helper",
            1_000,
            Duration::from_millis(100),
            true,
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("helper.pid");
        let hooks = RestartHooks {
            self_exe: std::env::current_exe,
            command: Box::new({
                let pid_file = pid_file.clone();
                move |p| {
                    let mut cmd = std::process::Command::new(p);
                    cmd.args([
                        "--exact",
                        "updater::tests::restart_helper_process",
                        "--test-threads=1",
                    ])
                    .env(HELPER_PID_FILE_ENV, &pid_file)
                    .env(HELPER_DELAY_ENV, delay_ms.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                    cmd
                }
            }),
            wait_ready: RestartHooks::default().wait_ready,
        };
        let host = Arc::new(FakeHost::default());
        let u = restart_updater(&host, b"test payload", hooks, timeout);
        u.0.check().await.expect("check");
        u.0.download_and_install().await.expect("install");
        let res = u.0.restart().await;
        if res.is_ok() {
            // The fake host keeps this process alive; stop the waiting
            // helper before it could swap the test binary.
            let pid: i32 = std::fs::read_to_string(&pid_file)
                .expect("pid file")
                .trim()
                .parse()
                .expect("pid");
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(pid).expect("pid"),
                rustix::process::Signal::KILL,
            );
        }
        if want_err {
            assert!(
                matches!(res, Err(UpdaterError::HelperNotReady(_))),
                "{name}: {res:?}"
            );
            assert_eq!(host.quits.load(Ordering::SeqCst), 0, "{name}");
        } else {
            assert_eq!(res, Ok(()), "{name}");
            assert_eq!(host.quits.load(Ordering::SeqCst), 1, "{name}");
        }
    }
}
