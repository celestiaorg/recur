//! Browser PKCE and headless device-code sign-in. One attempt at a time;
//! cancel, timeout and shutdown close the callback listener or stop polling
//! before they return.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use parking_lot::Mutex;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::auth::{
    Account, Auth, Credentials, OAUTH_CLIENT_ID, TokenResponse, or_cancel, random_secret,
};
use crate::oidc::IdentityClaims;

pub const LOGIN_BROWSER: &str = "browser";
pub const LOGIN_HEADLESS: &str = "headless";

const CANCELLED: &str = "ChatGPT sign-in was cancelled";

/// A pending sign-in: the link to open and, headless, the one-time code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Login {
    pub login_id: String,
    pub auth_url: String,
    pub method: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user_code: String,
}

pub(crate) struct Attempt {
    id: String,
    cancel: CancellationToken,
    done: CancellationToken,
    /// Written before `done` fires.
    err: Mutex<Option<String>>,
}

impl Attempt {
    fn finish(&self, res: Result<(), String>) {
        *self.err.lock() = res.err();
        self.cancel.cancel();
        self.done.cancel();
    }
}

type Previous = Option<Credentials>;

impl Auth {
    /// Starts a browser or headless sign-in. An empty `account_id` allows a
    /// new account; a saved id pins the selected account.
    pub async fn start_login(
        self: &Arc<Self>,
        account_id: &str,
        method: &str,
    ) -> Result<Login, String> {
        let method = if method.is_empty() {
            LOGIN_BROWSER
        } else {
            method
        };
        if method != LOGIN_BROWSER && method != LOGIN_HEADLESS {
            return Err("choose browser or headless ChatGPT sign-in".into());
        }
        let (p, previous) = {
            let mut st = self.state.lock().await;
            if st.closed {
                return Err("ChatGPT provider is shutting down".into());
            }
            if st.pending.as_ref().is_some_and(|p| !p.done.is_cancelled()) {
                return Err("ChatGPT sign-in is already in progress".into());
            }
            let mut s = self.read()?;
            let previous = if account_id.is_empty() {
                None
            } else {
                match s.account(account_id) {
                    Some(a) if a.client_id == OAUTH_CLIENT_ID => Some(a.clone()),
                    _ => return Err("unknown saved ChatGPT account; start a new sign-in".into()),
                }
            };
            let p = Arc::new(Attempt {
                id: random_secret(),
                cancel: CancellationToken::new(),
                done: CancellationToken::new(),
                err: Mutex::new(None),
            });
            st.pending = Some(Arc::clone(&p));
            (p, previous)
        };
        let timeout = Duration::from_secs(if method == LOGIN_HEADLESS { 15 } else { 5 } * 60);
        let cancel = p.cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(timeout) => cancel.cancel(),
                _ = cancel.cancelled() => {}
            }
        });
        if method == LOGIN_HEADLESS {
            return self.start_device_login(p, previous).await;
        }
        let (listener, redirect) = match self.listen_browser().await {
            Ok(l) => l,
            Err(e) => {
                p.finish(Err(e.clone()));
                return Err(e);
            }
        };
        let (state, verifier) = (random_secret(), random_secret());
        let challenge = URL_SAFE_NO_PAD.encode(aws_lc_rs::digest::digest(
            &aws_lc_rs::digest::SHA256,
            verifier.as_bytes(),
        ));
        let mut u = match url::Url::parse(&self.authorize_url) {
            Ok(u) => u,
            Err(e) => {
                p.finish(Err(e.to_string()));
                return Err(e.to_string());
            }
        };
        u.set_query(None);
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", OAUTH_CLIENT_ID)
                .append_pair("redirect_uri", &redirect)
                .append_pair("scope", "openid profile email offline_access")
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("id_token_add_organizations", "true")
                .append_pair("codex_cli_simplified_flow", "true")
                .append_pair("state", &state)
                .append_pair("originator", "recur");
            if let Some(email) = previous.as_ref().map(|a| &a.account.email)
                && !email.is_empty()
            {
                q.append_pair("login_hint", email);
            }
        }
        let login = Login {
            login_id: p.id.clone(),
            auth_url: u.to_string(),
            method: method.into(),
            ..Default::default()
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let res = match me.browser_callback(&p, listener, &state).await {
                Ok(code) => {
                    let grant = code_grant(&code, &redirect, &verifier);
                    me.finish_login(&p, &grant, previous).await
                }
                Err(e) => Err(e),
            };
            p.finish(res);
        });
        Ok(login)
    }

    async fn listen_browser(&self) -> Result<(TcpListener, String), String> {
        // One address: falling through to the other address family would share the port
        // with another application's listener.
        for &port in &self.callback_ports {
            if let Ok(l) = TcpListener::bind(("127.0.0.1", port)).await {
                let actual = l.local_addr().map_err(|e| e.to_string())?.port();
                return Ok((l, format!("http://localhost:{actual}/auth/callback")));
            }
        }
        Err("ChatGPT browser sign-in needs an available localhost port (1455 or 1457). Close another pending login or choose headless sign-in".into())
    }

    /// Serves the callback until one arrives with the right state; the
    /// listener closes when this returns.
    async fn browser_callback(
        &self,
        p: &Attempt,
        listener: TcpListener,
        state: &str,
    ) -> Result<String, String> {
        let (tx, mut rx) = mpsc::channel(1);
        let q = tokio::select! {
            _ = p.cancel.cancelled() => return Err("ChatGPT sign-in cancelled or timed out".into()),
            q = rx.recv() => q,
            () = serve_callback(listener, state, tx) => None,
        };
        let Some(q) = q else {
            return Err("ChatGPT callback listener stopped".into());
        };
        let get = |k: &str| {
            q.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        if !get("error").is_empty() || get("code").is_empty() {
            return Err("ChatGPT sign-in was declined or failed. Try signing in again".into());
        }
        Ok(get("code").to_owned())
    }

    async fn finish_login(
        &self,
        p: &Attempt,
        grant: &[(&str, &str)],
        previous: Previous,
    ) -> Result<(), String> {
        let tokens = or_cancel(&p.cancel, CANCELLED, self.exchange(grant)).await?;
        self.save_login(p, tokens, previous).await
    }

    async fn save_login(
        &self,
        p: &Attempt,
        tokens: TokenResponse,
        previous: Previous,
    ) -> Result<(), String> {
        let identity = or_cancel(
            &p.cancel,
            CANCELLED,
            self.validate_id_token(&tokens.id_token, OAUTH_CLIENT_ID),
        )
        .await?;
        let mut workspace = identity.account_id();
        if workspace.is_empty() {
            // Account routing may be only in the access token, which came
            // straight from the authenticated token endpoint.
            workspace = IdentityClaims::unverified(&tokens.access_token)
                .map(|c| c.account_id())
                .unwrap_or_default();
        }
        if workspace.is_empty() || tokens.refresh_token.is_empty() {
            return Err("ChatGPT did not return a renewable account session".into());
        }
        let id = credential_id(&identity.subject, &workspace);
        if previous
            .as_ref()
            .is_some_and(|a| a.account.id != id || a.subject != identity.subject)
        {
            return Err(
                "ChatGPT returned a different account. Choose another ChatGPT account to switch"
                    .into(),
            );
        }
        let st = self.state.lock().await;
        if !st.pending.as_ref().is_some_and(|c| std::ptr::eq(&**c, p))
            || p.cancel.is_cancelled()
            || st.closed
        {
            return Err(CANCELLED.into());
        }
        let mut s = self.read()?;
        let creds = Credentials {
            account: Account {
                id: id.clone(),
                email: identity.email,
            },
            subject: identity.subject,
            client_id: OAUTH_CLIENT_ID.into(),
            workspace_id: workspace,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            id_token: tokens.id_token,
            scope: tokens.scope,
            expires_at: Utc::now() + chrono::Duration::seconds(tokens.expires_in),
        };
        match s.account(&id) {
            Some(a) => *a = creds,
            None => s.accounts.push(creds),
        }
        s.active = id;
        self.write(&s)
    }

    /// Waits for the attempt; a cancelled or replaced one is an error.
    pub async fn wait_login(&self, login_id: &str) -> Result<(), String> {
        let p = self.state.lock().await.pending.clone();
        let Some(p) = p.filter(|p| p.id == login_id) else {
            return Err("ChatGPT sign-in is no longer pending".into());
        };
        p.done.cancelled().await;
        let mut st = self.state.lock().await;
        if !st.pending.as_ref().is_some_and(|c| Arc::ptr_eq(c, &p)) {
            return Err(CANCELLED.into());
        }
        st.pending = None;
        match p.err.lock().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Cancels only the given attempt and waits for it to stop.
    pub async fn cancel_login(&self, login_id: &str) {
        let p = {
            let mut st = self.state.lock().await;
            match st.pending.take_if(|p| p.id == login_id) {
                Some(p) => p,
                None => return,
            }
        };
        p.cancel.cancel();
        p.done.cancelled().await;
    }

    /// Stops any attempt and refuses further sign-ins and tokens.
    pub async fn close(&self) {
        let p = {
            let mut st = self.state.lock().await;
            st.closed = true;
            st.pending.take()
        };
        if let Some(p) = p {
            p.cancel.cancel();
            p.done.cancelled().await;
        }
    }

    async fn start_device_login(
        self: &Arc<Self>,
        p: Arc<Attempt>,
        previous: Previous,
    ) -> Result<Login, String> {
        let fail = |e: String| {
            p.finish(Err(e.clone()));
            Err(e)
        };
        let body = json!({"client_id": OAUTH_CLIENT_ID}).to_string();
        let raw = or_cancel(
            &p.cancel,
            CANCELLED,
            self.request(
                self.http
                    .post(&self.device_code_url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body),
            ),
        )
        .await;
        let raw = match raw {
            Ok(raw) => raw,
            Err(e) => return fail(format!("start headless sign-in: {e}")),
        };
        let Ok(mut code) = serde_json::from_slice::<DeviceCode>(&raw) else {
            return fail("invalid ChatGPT device authorization response".into());
        };
        if code.user_code.is_empty() {
            code.user_code = std::mem::take(&mut code.user_code_alias);
        }
        if code.auth_id.is_empty() || code.user_code.is_empty() {
            return fail("ChatGPT did not return a device code".into());
        }
        if p.cancel.is_cancelled() {
            return fail(CANCELLED.into());
        }
        let login = Login {
            login_id: p.id.clone(),
            auth_url: self.verification_url.clone(),
            method: LOGIN_HEADLESS.into(),
            user_code: code.user_code.clone(),
        };
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let res = match me.poll_device(&p.cancel, &code).await {
                Ok(tokens) => me.save_login(&p, tokens, previous).await,
                Err(e) => Err(e),
            };
            p.finish(res);
        });
        Ok(login)
    }

    pub(crate) async fn poll_device(
        &self,
        cancel: &CancellationToken,
        code: &DeviceCode,
    ) -> Result<TokenResponse, String> {
        let interval = device_interval(&code.interval) + self.polling_margin;
        let body = json!({"device_auth_id": code.auth_id, "user_code": code.user_code}).to_string();
        loop {
            let req = self
                .http
                .post(&self.device_token_url)
                .header(CONTENT_TYPE, "application/json")
                .header(USER_AGENT, "recur")
                .timeout(Duration::from_secs(30))
                .body(body.clone());
            let resp = tokio::select! {
                _ = cancel.cancelled() => return Err("ChatGPT device sign-in cancelled or timed out".into()),
                r = req.send() => r.map_err(|e| format!("ChatGPT device sign-in cancelled, expired, or unavailable: {e}"))?,
            };
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                #[derive(Default, Deserialize)]
                #[serde(default)]
                struct Authorized {
                    authorization_code: String,
                    code_verifier: String,
                }
                let raw = or_cancel(cancel, CANCELLED, async {
                    resp.bytes().await.map_err(|e| e.to_string())
                })
                .await?;
                let a = serde_json::from_slice::<Authorized>(&raw[..raw.len().min(1 << 20)])
                    .ok()
                    .filter(|a| !a.authorization_code.is_empty() && !a.code_verifier.is_empty())
                    .ok_or_else(|| "invalid ChatGPT device authorization result".to_owned())?;
                let redirect = format!("{}/deviceauth/callback", self.issuer);
                let grant = code_grant(&a.authorization_code, &redirect, &a.code_verifier);
                return or_cancel(cancel, CANCELLED, self.exchange(&grant)).await;
            }
            if status != 403 && status != 404 {
                return Err(format!(
                    "ChatGPT device authorization failed (HTTP {status}). Start a new sign-in"
                ));
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err("ChatGPT device sign-in cancelled or timed out".into()),
                _ = tokio::time::sleep(interval) => {}
            }
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DeviceCode {
    #[serde(rename = "device_auth_id")]
    pub auth_id: String,
    pub user_code: String,
    #[serde(rename = "usercode")]
    pub user_code_alias: String,
    pub interval: Value,
}

/// The polling interval: seconds as a string or number, 5 when absent, at
/// least 1.
fn device_interval(raw: &Value) -> Duration {
    let seconds = match raw {
        Value::String(s) => s.parse().unwrap_or(0),
        v => v.as_i64().unwrap_or(0),
    };
    let seconds = match seconds {
        0 => 5,
        s if s < 1 => 1,
        s => s,
    };
    Duration::from_secs(seconds as u64)
}

fn code_grant<'a>(code: &'a str, redirect: &'a str, verifier: &'a str) -> [(&'a str, &'a str); 5] {
    [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", OAUTH_CLIENT_ID),
        ("code_verifier", verifier),
    ]
}

fn credential_id(subject: &str, workspace: &str) -> String {
    let sum = aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        format!("{subject}\0{workspace}").as_bytes(),
    );
    recur_base::hex(&sum.as_ref()[..16])
}

/// Answers callback requests one at a time; a matching state is answered,
/// then handed to `tx`. Never returns unless accepting fails.
async fn serve_callback(
    listener: TcpListener,
    state: &str,
    tx: mpsc::Sender<Vec<(String, String)>>,
) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let handled = tokio::time::timeout(Duration::from_secs(5), async {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 << 10 {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let mut line = head.lines().next().unwrap_or_default().split(' ');
            let (method, target) = (line.next().unwrap_or_default(), line.next().unwrap_or_default());
            let u = url::Url::parse(&format!("http://localhost{target}")).ok();
            let (status, body, q) = match u {
                Some(u) if method == "GET" && u.path() == "/auth/callback" => {
                    let q: Vec<(String, String)> = u.query_pairs().into_owned().collect();
                    if q.iter().find(|(k, _)| k == "state").map(|(_, v)| v.as_str()) != Some(state) {
                        ("400 Bad Request", "Invalid sign-in state. Return to Recur and try again.\n", None)
                    } else {
                        ("200 OK", "You can return to Recur. It will confirm whether ChatGPT sign-in completed.", Some(q))
                    }
                }
                _ => ("404 Not Found", "404 page not found\n", None),
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
            q
        })
        .await;
        if let Ok(Some(q)) = handled {
            let _ = tx.try_send(q);
        }
    }
}
