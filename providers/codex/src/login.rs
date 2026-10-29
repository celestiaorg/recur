//! Recur-owned ChatGPT browser sign-in through app-server (Go
//! `app/codex_login.go`): `account/login/start`, the
//! `account/login/completed` notification, cancellation and shutdown.
//! Codex owns the localhost callback and writes `<CODEX_HOME>/auth.json`.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::ProviderStatus;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::rpc::{Ctx, RpcError};
use crate::server::{Server, ServerConfig, mark_browser_login};
use crate::status::login_method;
use crate::types::{LoginCompleted, Model, format_plan};

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A pending sign-in and the URL to open in the OS browser.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexLogin {
    pub login_id: String,
    pub auth_url: String,
}

/// A completed sign-in: the account card and the models it may use (`None`
/// when `model/list` failed).
#[derive(Debug, Clone, Default)]
pub struct LoginOutcome {
    pub status: ProviderStatus,
    pub models: Option<Vec<Model>>,
}

struct Session {
    id: String,
    cancel: CancellationToken,
    done: CancellationToken,
    /// Written by the worker before `done` fires.
    result: Mutex<Option<Result<LoginOutcome, String>>>,
}

#[derive(Default)]
struct State {
    cur: Option<Arc<Session>>,
    closed: bool,
}

/// At most one sign-in at a time; shut down with the app so the callback
/// listener cannot outlive Recur.
#[derive(Default)]
pub struct Logins {
    state: tokio::sync::Mutex<State>,
}

impl Logins {
    /// Starts app-server and its browser login; resolves once the URL is
    /// known or the attempt failed.
    pub async fn start(&self, cfg: ServerConfig) -> Result<CodexLogin, String> {
        self.start_with_timeout(cfg, LOGIN_TIMEOUT).await
    }

    pub(crate) async fn start_with_timeout(
        &self,
        cfg: ServerConfig,
        timeout: Duration,
    ) -> Result<CodexLogin, String> {
        let session = {
            let mut st = self.state.lock().await;
            if st.closed {
                return Err("Recur is shutting down".into());
            }
            if st.cur.as_ref().is_some_and(|s| !s.done.is_cancelled()) {
                return Err("Codex sign-in is already in progress".into());
            }
            let s = Arc::new(Session {
                id: uuid::Uuid::new_v4().to_string(),
                cancel: CancellationToken::new(),
                done: CancellationToken::new(),
                result: Mutex::new(None),
            });
            st.cur = Some(Arc::clone(&s));
            s
        };
        let (tx, rx) = oneshot::channel();
        tokio::spawn(session.run(cfg, timeout, tx));
        rx.await
            .unwrap_or_else(|_| Err("Codex sign-in stopped".into()))
    }

    /// Waits for the browser sign-in to finish.
    pub async fn wait(&self, login_id: &str) -> Result<LoginOutcome, String> {
        let session = self.state.lock().await.cur.clone();
        let Some(session) = session.filter(|s| s.id == login_id) else {
            return Err("Codex sign-in is no longer pending".into());
        };
        session.done.cancelled().await;
        let mut st = self.state.lock().await;
        if !st.cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, &session)) {
            return Err(RpcError::Canceled.to_string());
        }
        st.cur = None;
        session
            .result
            .lock()
            .take()
            .unwrap_or_else(|| Err(RpcError::Canceled.to_string()))
    }

    /// Cancels only the given attempt and reaps it before returning, so the
    /// old callback listener is gone before another attempt can start.
    pub async fn cancel(&self, login_id: &str) {
        let mut st = self.state.lock().await;
        if let Some(s) = st.cur.take_if(|s| s.id == login_id) {
            s.cancel.cancel();
            s.done.cancelled().await;
        }
    }

    /// Cancels any attempt and refuses new ones.
    pub async fn shutdown(&self) {
        let s = {
            let mut st = self.state.lock().await;
            st.closed = true;
            st.cur.take()
        };
        if let Some(s) = s {
            s.cancel.cancel();
            s.done.cancelled().await;
        }
    }
}

impl Session {
    async fn run(
        self: Arc<Self>,
        cfg: ServerConfig,
        timeout: Duration,
        started: oneshot::Sender<Result<CodexLogin, String>>,
    ) {
        let ctx = Ctx::new(self.cancel.clone()).with_timeout(timeout);
        let mut started = Some(started);
        let mut res = self.login(&cfg, &ctx, &mut started).await;
        if !self.cancel.is_cancelled()
            && ctx
                .deadline
                .is_some_and(|d| tokio::time::Instant::now() >= d)
        {
            res = Err("Codex sign-in timed out. Try again".into());
        }
        self.cancel.cancel();
        let err = res.as_ref().err().cloned();
        *self.result.lock() = Some(res);
        self.done.cancel();
        if let (Some(tx), Some(e)) = (started.take(), err) {
            let _ = tx.send(Err(e));
        }
    }

    async fn login(
        &self,
        cfg: &ServerConfig,
        ctx: &Ctx,
        started: &mut Option<oneshot::Sender<Result<CodexLogin, String>>>,
    ) -> Result<LoginOutcome, String> {
        let srv = Server::start(cfg, ctx).await?;
        let out = self.drive(&srv, cfg, ctx, started).await;
        srv.close().await;
        out
    }

    async fn drive(
        &self,
        srv: &Server,
        cfg: &ServerConfig,
        ctx: &Ctx,
        started: &mut Option<oneshot::Sender<Result<CodexLogin, String>>>,
    ) -> Result<LoginOutcome, String> {
        let client = Client::new(srv.conn().clone());
        let (tx, mut completed) = mpsc::unbounded_channel();
        srv.conn()
            .on_notification(Some(Arc::new(move |method: &str, params: &str| {
                if method == "account/login/completed"
                    && let Ok(c) = serde_json::from_str::<LoginCompleted>(params)
                {
                    let _ = tx.send(c);
                }
            })));
        let login = client
            .account_login_start(ctx)
            .await
            .map_err(|e| format!("start Codex sign-in: {e}"))?;
        if login.login_id.is_empty() || login.auth_url.is_empty() {
            return Err("Codex did not return a browser sign-in URL".into());
        }
        if let Some(tx) = started.take() {
            let _ = tx.send(Ok(CodexLogin {
                login_id: self.id.clone(),
                auth_url: login.auth_url,
            }));
        }
        let stop = ctx.run(std::future::pending::<Result<(), RpcError>>());
        tokio::pin!(stop);
        loop {
            tokio::select! {
                r = &mut stop => {
                    let quick = Ctx::default().with_timeout(Duration::from_secs(2));
                    let _ = client.account_login_cancel(&quick, &login.login_id).await;
                    return Err(r.err().unwrap_or(RpcError::Canceled).to_string());
                }
                _ = srv.reaped() => return Err("Codex exited during sign-in".into()),
                Some(result) = completed.recv() => {
                    if result.login_id != login.login_id {
                        continue;
                    }
                    if !result.success {
                        return Err(format!("Codex sign-in failed: {}", result.error));
                    }
                    let acct = client.account_read(ctx).await.map_err(|e| e.to_string())?;
                    let Some(account) = acct.account.filter(|a| a.typ == "chatgpt") else {
                        return Err("Codex did not authenticate a ChatGPT account".into());
                    };
                    mark_browser_login(&cfg.home).map_err(|e| e.to_string())?;
                    let models = client.model_list(ctx, 50).await.ok();
                    return Ok(LoginOutcome {
                        status: ProviderStatus {
                            installed: true,
                            path: cfg.bin.clone(),
                            authenticated: true,
                            login_method: login_method(&account.typ, false),
                            email: account.email,
                            plan: format_plan(&account.plan_type),
                            ..Default::default()
                        },
                        models,
                    });
                }
            }
        }
    }
}

/// A fake `codex app-server` for sign-in tests, run with `dir` as the server's
/// working directory. `<dir>/mode` picks the outcome of `account/login/start`:
/// `success`, `failed`, `pending` (no completion) or `exit`.
///
/// [`locate`](crate::locate::locate) accepts only native binaries, so `codex`
/// is a symlink to `/bin/sh`, which runs `codex app-server …` as the script
/// `<dir>/app-server`.
#[cfg(any(test, feature = "testing"))]
pub fn fake_login_server(dir: &std::path::Path, mode: &str) -> std::path::PathBuf {
    let bin = dir.join("codex");
    std::fs::write(dir.join("mode"), mode).expect("write mode");
    let script = format!(
        r#"#!/bin/sh
mode_file='{mode}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialized"'*) continue ;;
    *'"method":"account/login/start"'*)
      mode=$(cat "$mode_file")
      echo "{{\"id\":$id,\"result\":{{\"loginId\":\"codex-login\",\"authUrl\":\"https://auth.openai.com/test-login\"}}}}"
      case "$mode" in
        exit) exit 0 ;;
        pending) continue ;;
        success) printf '%s' '{{"tokens":{{"access_token":"recur-test"}}}}' > "$CODEX_HOME/auth.json" ;;
      esac
      ok=false
      [ "$mode" = success ] && ok=true
      echo "{{\"method\":\"account/login/completed\",\"params\":{{\"loginId\":\"codex-login\",\"success\":$ok,\"error\":\"Login rejected\"}}}}"
      continue ;;
    *'"method":"account/login/cancel"'*)
      printf codex-login > "$CODEX_HOME/cancelled"
      result='{{}}' ;;
    *'"method":"account/read"'*)
      if [ -f "$CODEX_HOME/auth.json" ]; then
        result='{{"account":{{"type":"chatgpt","email":"recur@example.com","planType":"plus"}}}}'
      else
        result='{{}}'
      fi ;;
    *'"method":"model/list"'*) result='{{"data":[{{"id":"gpt-test","displayName":"GPT Test"}}]}}' ;;
    *) result='{{}}' ;;
  esac
  echo "{{\"id\":$id,\"result\":$result}}"
done
"#,
        mode = dir.join("mode").display()
    );
    std::fs::write(dir.join("app-server"), script).expect("write fake app-server");
    std::os::unix::fs::symlink("/bin/sh", &bin).expect("link fake codex");
    bin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(mode: &str) -> (tempfile::TempDir, ServerConfig) {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_login_server(dir.path(), mode);
        let cfg = ServerConfig {
            bin: bin.to_string_lossy().into_owned(),
            home: dir.path().join("codex-home"),
            worktree: dir.path().to_path_buf(),
            config_overrides: vec![r#"cli_auth_credentials_store="file""#.into()],
            ..Default::default()
        };
        (dir, cfg)
    }

    #[tokio::test]
    async fn browser_login() {
        let (_dir, cfg) = fixture("success");
        let logins = Logins::default();
        let login = logins.start(cfg.clone()).await.unwrap();
        assert_eq!(login.auth_url, "https://auth.openai.com/test-login");
        assert!(!login.login_id.is_empty());
        let out = logins.wait(&login.login_id).await.unwrap();
        assert!(out.status.authenticated);
        assert_eq!(out.status.email, "recur@example.com");
        assert_eq!(out.models.unwrap()[0].id, "gpt-test");
        assert!(cfg.home.join("auth.json").exists());
        assert!(cfg.home.join("recur-browser-login").exists());
    }

    #[tokio::test]
    async fn failure_and_retry() {
        let (dir, cfg) = fixture("failed");
        let logins = Logins::default();
        let login = logins.start(cfg.clone()).await.unwrap();
        let err = logins.wait(&login.login_id).await.unwrap_err();
        assert!(err.contains("Login rejected"), "{err}");
        std::fs::write(dir.path().join("mode"), "success").unwrap();
        let login = logins.start(cfg).await.unwrap();
        logins.wait(&login.login_id).await.expect("retry failed");
    }

    #[tokio::test]
    async fn cancel_and_shutdown() {
        let (_dir, cfg) = fixture("pending");
        let logins = Logins::default();
        let login = logins.start(cfg.clone()).await.unwrap();
        assert!(
            logins.start(cfg.clone()).await.is_err(),
            "allowed overlapping login attempts"
        );
        logins.cancel("older-attempt").await;
        logins.cancel(&login.login_id).await;
        assert_eq!(
            std::fs::read_to_string(cfg.home.join("cancelled")).unwrap(),
            "codex-login",
            "Codex did not receive login cancellation"
        );
        assert!(logins.wait(&login.login_id).await.is_err());
        logins
            .start(cfg.clone())
            .await
            .expect("could not restart after cancellation");
        logins.shutdown().await;
        assert!(
            logins.start(cfg).await.is_err(),
            "started login after shutdown"
        );
    }

    #[tokio::test]
    async fn timeout_and_exit() {
        for mode in ["pending", "exit"] {
            let (_dir, cfg) = fixture(mode);
            let logins = Logins::default();
            let login = logins
                .start_with_timeout(cfg, Duration::from_secs(1))
                .await
                .unwrap();
            let err = tokio::time::timeout(Duration::from_secs(5), logins.wait(&login.login_id))
                .await
                .expect("login worker did not stop")
                .unwrap_err();
            assert_ne!(err, "context canceled", "{mode}");
        }
    }
}
