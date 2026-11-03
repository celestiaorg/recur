//! ChatGPT client and provider tests against a signed local OAuth server and a
//! streamed Responses fixture.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::{KeyPair as _, RSA_PKCS1_SHA256};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use parking_lot::Mutex;
use recur_agent::{AgentEvent, EventSink, Provider, ToolHost, TurnContext, TurnRequest};
use recur_domain::{CompatibleModel, Settings};
use recur_provider_compatible::HarnessTools;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::auth::{AuthFile, Credentials, OAUTH_CLIENT_ID, PROTOCOL_VERSION};
use crate::login::DeviceCode;

/// One parsed request.
#[derive(Debug, Clone, Default)]
struct Req {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> String {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn form(&self) -> Vec<(String, String)> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .into_owned()
            .collect()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_default()
    }
}

fn get(pairs: &[(String, String)], k: &str) -> String {
    pairs
        .iter()
        .find(|(key, _)| key == k)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

type Handler = Arc<dyn Fn(&Req) -> (u16, String) + Send + Sync>;

/// A one-request-per-connection HTTP/1.1 server; returns its base URL.
async fn serve(handler: Handler) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let req = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap_or_default().split(' ');
                    let method = first.next().unwrap_or_default().to_owned();
                    let target = first.next().unwrap_or_default();
                    let u = url::Url::parse(&format!("http://x{target}")).unwrap();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                        .collect();
                    let len: usize = get(
                        &headers
                            .iter()
                            .map(|(k, v)| (k.to_lowercase(), v.clone()))
                            .collect::<Vec<_>>(),
                        "content-length",
                    )
                    .parse()
                    .unwrap_or(0);
                    while buf.len() < end + 4 + len {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    break Req {
                        method,
                        path: u.path().to_owned(),
                        query: u.query_pairs().into_owned().collect(),
                        headers,
                        body: String::from_utf8_lossy(&buf[end + 4..]).into_owned(),
                    };
                };
                let (status, body) = handler(&req);
                let ct = if body.starts_with("data:") {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    base
}

#[derive(Default)]
struct Knobs {
    challenge: String,
    redirect: String,
    subject: String,
    workspace: String,
    token_fault: String,
    pending_statuses: Vec<u16>,
    device_failure: u16,
}

struct Fixture {
    auth: Arc<Auth>,
    issuer: String,
    key: KeyPair,
    knobs: Mutex<Knobs>,
    exchanges: AtomicUsize,
    refreshes: AtomicUsize,
    polls: AtomicUsize,
    _dir: tempfile::TempDir,
}

/// `n` and `e` of a DER `RSAPublicKey` (`SEQUENCE { INTEGER, INTEGER }`).
fn rsa_public_numbers(der: &[u8]) -> (&[u8], &[u8]) {
    fn tlv(b: &[u8]) -> (&[u8], &[u8]) {
        let (len, off) = match b[1] {
            l if l < 0x80 => (l as usize, 2),
            l => {
                let n = (l & 0x7f) as usize;
                (
                    b[2..2 + n].iter().fold(0, |a, &x| a << 8 | x as usize),
                    2 + n,
                )
            }
        };
        (&b[off..off + len], &b[off + len..])
    }
    let strip = |v: &[u8]| -> usize { v.iter().take_while(|&&x| x == 0).count() };
    let (seq, _) = tlv(der);
    let (n, rest) = tlv(seq);
    let (e, _) = tlv(rest);
    (&n[strip(n)..], &e[strip(e)..])
}

fn b64(v: &Value) -> String {
    URL_SAFE_NO_PAD.encode(v.to_string())
}

impl Fixture {
    async fn new() -> Arc<Self> {
        Self::with(|_| {}).await
    }

    async fn with(tune: impl FnOnce(&mut Auth)) -> Arc<Self> {
        let dir = tempfile::tempdir().unwrap();
        let slot: Arc<Mutex<Option<std::sync::Weak<Fixture>>>> = Arc::default();
        let weak = Arc::clone(&slot);
        let base = serve(Arc::new(move |req: &Req| {
            let f = weak.lock().as_ref().and_then(|w| w.upgrade());
            f.map_or((503, String::new()), |f| f.answer(req))
        }))
        .await;
        let mut auth = Auth::new(dir.path().to_path_buf());
        auth.issuer = base.clone();
        auth.authorize_url = format!("{base}/oauth/authorize");
        auth.token_url = format!("{base}/oauth/token");
        auth.jwks_url = format!("{base}/jwks");
        auth.models_url = format!("{base}/models?client_version={PROTOCOL_VERSION}");
        auth.device_code_url = format!("{base}/deviceauth/usercode");
        auth.device_token_url = format!("{base}/deviceauth/token");
        auth.verification_url = format!("{base}/codex/device");
        auth.callback_ports = vec![0];
        auth.polling_margin = Duration::ZERO;
        tune(&mut auth);
        let f = Arc::new(Self {
            auth: Arc::new(auth),
            issuer: base,
            key: KeyPair::generate(KeySize::Rsa2048).unwrap(),
            knobs: Mutex::new(Knobs {
                subject: "user-one".into(),
                workspace: "workspace-one".into(),
                ..Default::default()
            }),
            exchanges: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            polls: AtomicUsize::new(0),
            _dir: dir,
        });
        *slot.lock() = Some(Arc::downgrade(&f));
        f
    }

    fn sign(&self, claims: &Value) -> String {
        let payload = format!(
            "{}.{}",
            b64(&json!({"alg": "RS256", "kid": "test-key"})),
            b64(claims)
        );
        let mut sig = vec![0u8; self.key.public_modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &aws_lc_rs::rand::SystemRandom::new(),
                payload.as_bytes(),
                &mut sig,
            )
            .unwrap();
        format!("{payload}.{}", URL_SAFE_NO_PAD.encode(sig))
    }

    fn answer(&self, r: &Req) -> (u16, String) {
        let k = self.knobs.lock();
        match r.path.as_str() {
            "/jwks" => {
                let (n, e) = rsa_public_numbers(self.key.public_key().as_ref());
                let key = json!({"kid": "test-key", "kty": "RSA", "alg": "RS256", "use": "sig",
                    "n": URL_SAFE_NO_PAD.encode(n), "e": URL_SAFE_NO_PAD.encode(e)});
                (200, json!({"keys": [key]}).to_string())
            }
            "/oauth/token" => {
                let form = r.form();
                assert!(
                    get(&form, "client_id") == OAUTH_CLIENT_ID
                        && !form.iter().any(|(k, _)| k == "resource"),
                    "token exchange did not use the shared Codex client"
                );
                if get(&form, "grant_type") == "refresh_token" {
                    self.refreshes.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        get(&form, "refresh_token"),
                        "refresh-one",
                        "refresh token was not serialized"
                    );
                    return (
                        200,
                        json!({"access_token": "access-two", "refresh_token": "refresh-two", "expires_in": 3600}).to_string(),
                    );
                }
                self.exchanges.fetch_add(1, Ordering::SeqCst);
                if get(&form, "code") == "device-code" {
                    assert!(
                        get(&form, "code_verifier") == "device-verifier"
                            && get(&form, "redirect_uri")
                                == format!("{}/deviceauth/callback", self.issuer),
                        "wrong device code exchange"
                    );
                } else {
                    let sum = aws_lc_rs::digest::digest(
                        &aws_lc_rs::digest::SHA256,
                        get(&form, "code_verifier").as_bytes(),
                    );
                    assert!(
                        URL_SAFE_NO_PAD.encode(sum) == k.challenge
                            && get(&form, "redirect_uri") == k.redirect,
                        "PKCE or callback URI did not match"
                    );
                }
                let mut claims = json!({"iss": self.issuer, "aud": OAUTH_CLIENT_ID, "sub": k.subject,
                    "email": "person@example.com", "exp": chrono::Utc::now().timestamp() + 3600,
                    "https://api.openai.com/auth": {"chatgpt_account_id": k.workspace}});
                match k.token_fault.as_str() {
                    "issuer" => claims["iss"] = "https://wrong.invalid".into(),
                    "audience" => claims["aud"] = "wrong-client".into(),
                    "expired" => claims["exp"] = (chrono::Utc::now().timestamp() - 60).into(),
                    "account" => {
                        claims
                            .as_object_mut()
                            .unwrap()
                            .remove("https://api.openai.com/auth");
                    }
                    _ => {}
                }
                let mut id_token = self.sign(&claims);
                if k.token_fault == "signature" {
                    id_token.push_str("broken");
                }
                // Codex OAuth responses need not carry token_type, scope or expires_in.
                (
                    200,
                    json!({"access_token": "access-one", "refresh_token": "refresh-one", "id_token": id_token}).to_string(),
                )
            }
            "/deviceauth/usercode" => {
                assert_eq!(
                    r.json()["client_id"],
                    OAUTH_CLIENT_ID,
                    "device login used an agent client"
                );
                (
                    200,
                    r#"{"device_auth_id":"private-device-id","user_code":"ABCD-1234","interval":"1"}"#
                        .into(),
                )
            }
            "/deviceauth/token" => {
                let body = r.json();
                assert!(
                    body["device_auth_id"] == "private-device-id"
                        && body["user_code"] == "ABCD-1234",
                    "wrong device polling body"
                );
                let n = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
                if k.device_failure != 0 {
                    return (k.device_failure, String::new());
                }
                if n <= k.pending_statuses.len() {
                    return (k.pending_statuses[n - 1], String::new());
                }
                (
                    200,
                    r#"{"authorization_code":"device-code","code_verifier":"device-verifier"}"#
                        .into(),
                )
            }
            "/models" => {
                assert!(
                    r.header("Authorization").starts_with("Bearer access-")
                        && r.header("ChatGPT-Account-Id") == k.workspace
                        && r.header("Originator") == "recur"
                        && !get(&r.query, "client_version").is_empty(),
                    "model request missing Codex account routing"
                );
                (
                    200,
                    r#"{"models":[{"slug":"hidden","visibility":"hide"},{"slug":"gpt-first","display_name":"First","visibility":"list"},{"slug":"gpt-second","display_name":"Second","visibility":"list"}]}"#.into(),
                )
            }
            _ => (404, String::new()),
        }
    }

    async fn start(&self, account: &str) -> (Login, Vec<(String, String)>) {
        let login = self.auth.start_login(account, LOGIN_BROWSER).await.unwrap();
        let u = url::Url::parse(&login.auth_url).unwrap();
        let q: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        let mut k = self.knobs.lock();
        k.challenge = get(&q, "code_challenge");
        k.redirect = get(&q, "redirect_uri");
        (login, q)
    }

    async fn callback(&self, q: &[(String, String)], state: &str) -> u16 {
        let mut u = url::Url::parse(&get(q, "redirect_uri")).unwrap();
        u.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("code", "test-code");
        reqwest::get(u).await.unwrap().status().as_u16()
    }

    async fn connect(&self) -> String {
        let (login, q) = self.start("").await;
        self.callback(&q, &get(&q, "state")).await;
        self.auth.wait_login(&login.login_id).await.unwrap();
        self.auth.status().await.unwrap().account_id
    }
}

#[test]
fn codex_oauth_endpoints() {
    let a = Auth::new(std::env::temp_dir());
    assert!(
        a.authorize_url == "https://auth.openai.com/oauth/authorize"
            && a.token_url == "https://auth.openai.com/oauth/token",
        "browser OAuth is not using the Codex endpoints"
    );
    assert!(
        a.verification_url == "https://auth.openai.com/codex/device"
            && a.device_code_url == "https://auth.openai.com/api/accounts/deviceauth/usercode"
            && a.device_token_url == "https://auth.openai.com/api/accounts/deviceauth/token",
        "headless OAuth is not using the Codex device flow"
    );
    assert!(
        a.models_url
            .starts_with(&format!("{}/models?client_version=", auth::BASE_URL)),
        "models use the wrong account API"
    );
}

#[tokio::test]
async fn browser_login_falls_back_without_cancelling_other_applications() {
    let busy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();
    let f = Fixture::with(|a| a.callback_ports = vec![port, 0]).await;
    let (login, q) = f.start("").await;
    let u = url::Url::parse(&get(&q, "redirect_uri")).unwrap();
    assert_ne!(
        u.port(),
        Some(port),
        "reused another application's listener"
    );
    f.auth.cancel_login(&login.login_id).await;
}

#[tokio::test]
async fn browser_login_uses_codex_flow_without_agent_registration() {
    let f = Fixture::new().await;
    let (login, q) = f.start("").await;
    assert!(
        get(&q, "client_id") == OAUTH_CLIENT_ID
            && get(&q, "scope") == "openid profile email offline_access"
            && get(&q, "originator") == "recur"
            && get(&q, "codex_cli_simplified_flow") == "true"
            && get(&q, "id_token_add_organizations") == "true",
        "wrong Codex browser flow"
    );
    for key in ["agent_name_hint", "ext_agent_host_id", "resource"] {
        assert!(
            !q.iter().any(|(k, _)| k == key),
            "agent-registration parameter {key} was sent"
        );
    }
    let redirect = url::Url::parse(&get(&q, "redirect_uri")).unwrap();
    assert!(
        redirect.host_str() == Some("localhost") && redirect.path() == "/auth/callback",
        "incorrect Codex callback URI"
    );
    assert!(
        f.callback(&q, "wrong-state").await == 400 && f.exchanges.load(Ordering::SeqCst) == 0,
        "invalid state reached the token exchange"
    );
    f.callback(&q, &get(&q, "state")).await;
    f.auth.wait_login(&login.login_id).await.unwrap();
    let status = f.auth.status().await.unwrap();
    assert!(
        status.authenticated && !status.account_id.is_empty(),
        "{status:?}"
    );
    let public = serde_json::to_string(&status).unwrap();
    assert!(
        !public.contains("access-one") && !public.contains("refresh-one"),
        "tokens leaked into status"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(f.auth.dir.join("chatgpt-auth.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "credential permissions");
    let models = f.auth.models(&status.account_id).await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-first");
    let token = Auth::new(f.auth.dir.clone())
        .access_token(&status.account_id)
        .await
        .expect("restart token");
    assert_eq!(token, "access-one");
    let (login, returning) = f.start(&status.account_id).await;
    assert!(
        get(&returning, "client_id") == OAUTH_CLIENT_ID
            && get(&returning, "login_hint") == "person@example.com"
            && get(&returning, "state") != get(&q, "state")
            && get(&returning, "code_challenge") != get(&q, "code_challenge"),
        "returning login reused secrets or registered an agent"
    );
    f.auth.cancel_login(&login.login_id).await;
}

#[tokio::test]
async fn headless_login_polls_and_exchanges_without_listener() {
    // Any attempt to bind a browser listener would fail.
    let f = Fixture::with(|a| a.callback_ports = Vec::new()).await;
    f.knobs.lock().pending_statuses = vec![403, 404];
    let login = f.auth.start_login("", LOGIN_HEADLESS).await.unwrap();
    assert!(
        login.auth_url == f.auth.verification_url
            && login.user_code == "ABCD-1234"
            && login.method == LOGIN_HEADLESS,
        "device link/code not returned"
    );
    let public = serde_json::to_string(&login).unwrap();
    assert!(
        !public.contains("private-device-id"),
        "private device ID leaked into settings"
    );
    f.auth.wait_login(&login.login_id).await.unwrap();
    let status = f.auth.status().await.unwrap();
    assert!(
        status.authenticated
            && f.polls.load(Ordering::SeqCst) == 3
            && f.exchanges.load(Ordering::SeqCst) == 1,
        "device flow did not complete: {status:?}"
    );
}

#[tokio::test]
async fn login_validates_identity() {
    for fault in ["signature", "issuer", "audience", "expired", "account"] {
        let f = Fixture::new().await;
        f.knobs.lock().token_fault = fault.into();
        let (login, q) = f.start("").await;
        f.callback(&q, &get(&q, "state")).await;
        assert!(
            f.auth.wait_login(&login.login_id).await.is_err(),
            "{fault}: invalid identity was accepted"
        );
        assert!(
            !f.auth.status().await.unwrap().authenticated,
            "{fault}: failed login activated credentials"
        );
    }
}

#[tokio::test]
async fn refresh_is_serialized_and_rotated() {
    let f = Fixture::new().await;
    let id = f.connect().await;
    let mut s = f.auth.read().unwrap();
    s.account(&id).unwrap().expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    f.auth.write(&s).unwrap();
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (auth, id) = (Arc::clone(&f.auth), id.clone());
        set.spawn(async move { auth.access_token(&id).await });
    }
    while let Some(r) = set.join_next().await {
        assert_eq!(r.unwrap().expect("refresh failed"), "access-two");
    }
    assert_eq!(f.refreshes.load(Ordering::SeqCst), 1);
    let mut s = f.auth.read().unwrap();
    assert_eq!(
        s.account(&id).unwrap().refresh_token,
        "refresh-two",
        "rotated token was not persisted"
    );
    f.auth.disconnect().await.unwrap();
    assert!(
        !f.auth.status().await.unwrap().authenticated,
        "sign-out did not clear credentials"
    );
}

#[tokio::test]
async fn cancelled_and_foreign_account_login_preserve_active_account() {
    let f = Fixture::new().await;
    let id = f.connect().await;
    let (login, _) = f.start("").await;
    f.auth.cancel_login(&login.login_id).await;
    assert!(
        f.auth.wait_login(&login.login_id).await.is_err(),
        "cancelled attempt succeeded"
    );
    let (login, q) = f.start(&id).await;
    f.knobs.lock().subject = "a-different-user".into();
    f.callback(&q, &get(&q, "state")).await;
    assert!(
        f.auth.wait_login(&login.login_id).await.is_err(),
        "changed account identity was accepted"
    );
    assert_eq!(
        f.auth.access_token(&id).await.as_deref(),
        Ok("access-one"),
        "failed switch changed the active session"
    );
}

#[tokio::test]
async fn device_failure_and_cancellation_keep_active_account() {
    for cancel in [false, true] {
        let f = Fixture::new().await;
        let id = f.connect().await;
        if cancel {
            f.knobs.lock().pending_statuses = vec![403];
        } else {
            f.knobs.lock().device_failure = 400;
        }
        let login = f.auth.start_login("", LOGIN_HEADLESS).await.unwrap();
        if cancel {
            f.auth.cancel_login(&login.login_id).await;
        }
        assert!(
            f.auth.wait_login(&login.login_id).await.is_err(),
            "failed device attempt succeeded"
        );
        let status = f.auth.status().await.unwrap();
        assert!(
            status.authenticated && status.account_id == id,
            "device failure replaced the active account"
        );
    }
}

#[tokio::test]
async fn login_cancellation_closes_listener() {
    let f = Fixture::new().await;
    let (login, q) = f.start("").await;
    f.auth.cancel_login(&login.login_id).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    assert!(
        client.get(get(&q, "redirect_uri")).send().await.is_err(),
        "cancelled listener is still accepting callbacks"
    );
}

#[tokio::test]
async fn device_polling_timeout() {
    let f = Fixture::new().await;
    f.knobs.lock().pending_statuses = vec![403];
    let cancel = CancellationToken::new();
    let timer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        timer.cancel();
    });
    let code = DeviceCode {
        auth_id: "private-device-id".into(),
        user_code: "ABCD-1234".into(),
        interval: json!("1"),
        ..Default::default()
    };
    assert!(
        f.auth.poll_device(&cancel, &code).await.is_err(),
        "polling did not respect timeout"
    );
}

#[tokio::test]
async fn agent_credentials_are_not_reused_for_codex() {
    let f = Fixture::new().await;
    f.auth
        .write(&AuthFile {
            active: "old-agent".into(),
            accounts: vec![Credentials {
                account: Account {
                    id: "old-agent".into(),
                    ..Default::default()
                },
                access_token: "agent-token".into(),
                refresh_token: "agent-refresh".into(),
                scope: "chatgpt.tokens.use.direct".into(),
                ..Default::default()
            }],
        })
        .unwrap();
    let status = f.auth.status().await.unwrap();
    assert!(
        !status.authenticated && status.accounts.is_empty(),
        "old agent credentials appeared as a Codex login"
    );
    assert!(
        f.auth.access_token("old-agent").await.is_err(),
        "agent token was reused for Codex"
    );
}

#[tokio::test]
async fn oauth_errors_do_not_expose_token_bodies() {
    let base = serve(Arc::new(|_: &Req| (401, "secret-token-body".into()))).await;
    let f = Fixture::with(|a| a.token_url = base).await;
    let err = f
        .auth
        .exchange(&[("client_id", OAUTH_CLIENT_ID)])
        .await
        .unwrap_err();
    assert!(
        !err.contains("secret-token-body"),
        "token response leaked: {err}"
    );
}

fn chatgpt_settings() -> Settings {
    Settings {
        chat_gpt_account_id: "registration".into(),
        chat_gpt_models: vec![CompatibleModel {
            id: "gpt-test".into(),
            label: "GPT Test".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn chatgpt_uses_native_harness_without_cli_or_mcp() {
    let p = ChatGPTProvider::new(std::env::temp_dir(), chatgpt_settings());
    assert_eq!(p.availability(&chatgpt_settings()), Availability::Ready);
    let caps = p.caps();
    assert_eq!(
        (caps.tool_transport, caps.history),
        (
            recur_agent::ToolTransport::InProcess,
            recur_agent::HistoryMode::App
        ),
        "direct ChatGPT would start an MCP server or a CLI"
    );
    let client =
        Client::new(&chatgpt_settings().chatgpt_provider()).with_chatgpt(Arc::new(Bound {
            auth: Arc::clone(p.auth()),
            account: "registration".into(),
        }));
    assert_eq!(
        client.endpoint(),
        "https://chatgpt.com/backend-api/codex/responses",
        "ChatGPT is not using the Codex subscription endpoint"
    );
    assert_eq!(
        p.availability(&Settings::default()),
        Availability::Unavailable("connect ChatGPT in Settings → Providers → OpenAI".into())
    );
}

/// Sets fixed OAuth headers and counts calls.
struct Fixed(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Authorize for Fixed {
    async fn authorize(&self, h: &mut HeaderMap) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        h.insert(
            "Authorization",
            "Bearer direct-oauth-token".parse().unwrap(),
        );
        h.insert("ChatGPT-Account-Id", "workspace-one".parse().unwrap());
        h.insert("Originator", "recur".parse().unwrap());
        Ok(())
    }
}

#[tokio::test]
async fn chatgpt_streams_tool_round_and_replays_reasoning_without_codex() {
    let worktree = tempfile::tempdir().unwrap();
    std::fs::write(worktree.path().join("hello.txt"), "hello from a real file").unwrap();
    let rounds = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&rounds);
    let base = serve(Arc::new(move |r: &Req| {
        assert!(
            r.path == "/backend-api/codex/responses"
                && r.method == "POST"
                && r.header("Authorization") == "Bearer direct-oauth-token"
                && r.header("ChatGPT-Account-Id") == "workspace-one"
                && r.header("Originator") == "recur",
            "wrong endpoint or OAuth authorization"
        );
        let body = r.json();
        assert!(
            body["stream"] == true && body["store"] == false,
            "ChatGPT request was not stateless streaming"
        );
        assert_eq!(
            body["include"],
            json!(["reasoning.encrypted_content"]),
            "stateless tool rounds did not request encrypted reasoning"
        );
        assert!(body.get("temperature").is_none(), "unsupported ChatGPT request field");
        let tools = body["tools"].as_array().expect("missing function tools");
        assert_eq!(tools.len(), 1, "missing function tools");
        assert!(
            tools[0]["type"] == "function" && tools[0]["name"] == "Read",
            "Codex request changed the tool schema"
        );
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            return (200, concat!(
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\",\"arguments\":\"{\\\"file_path\\\":\\\"hello.txt\\\"}\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_one\",\"status\":\"completed\",\"output\":[{\"id\":\"reasoning_one\",\"type\":\"reasoning\",\"summary\":[],\"encrypted_content\":\"encrypted-reasoning\"},{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\",\"arguments\":\"{\\\"file_path\\\":\\\"hello.txt\\\"}\"}]}}\n\n",
            ).into());
        }
        let input = body["input"].to_string();
        assert!(
            input.contains("encrypted-reasoning")
                && input.contains("hello from a real file")
                && !input.contains(r#""namespace":"recur""#),
            "next round dropped reasoning/tool output or rewrote the function call: {input}"
        );
        (200, concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Read the file\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_two\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Read the file\"}]}]}}\n\n",
        ).into())
    }))
    .await;
    let mut config = chatgpt_settings().chatgpt_provider();
    config.base_url = format!("{base}/backend-api/codex");
    let auth_calls = Arc::new(AtomicUsize::new(0));
    let client = Client::new(&config).with_chatgpt(Arc::new(Fixed(Arc::clone(&auth_calls))));
    let req = TurnRequest {
        model: "gpt-test".into(),
        prompt: "Read hello.txt".into(),
        worktree: worktree.path().into(),
        allowed_tools: Some(vec!["Read".into()]),
        include_partials: true,
        ..Default::default()
    };
    let streamed: Arc<Mutex<String>> = Arc::default();
    let log = Arc::clone(&streamed);
    let sink: Arc<dyn EventSink> = Arc::new(move |ev: AgentEvent| {
        if let AgentEvent::TextDelta { text, .. } = ev {
            log.lock().push_str(&text);
        }
    });
    let mut cx = TurnContext::new(CancellationToken::new(), sink);
    cx.tools = Some(Arc::new(HarnessTools::new(None, Arc::default(), &req)) as Arc<dyn ToolHost>);
    let result = run_harness(&config, &client, None, req, cx).await.unwrap();
    assert_eq!(result.text, "Read the file");
    assert_eq!(*streamed.lock(), result.text);
    assert_eq!(rounds.load(Ordering::SeqCst), 2);
    assert_eq!(auth_calls.load(Ordering::SeqCst), 2);
}
