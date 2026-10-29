//! Cursor usage Connect: the browser sign-in that mints Recur-owned tokens,
//! and the local-only rows the footer shows around it (Go `usage/connect.go`).

use std::sync::Arc;

use crate::ctx::{Ctx, CtxError};
use crate::cursor::fetch_cursor_email;
use crate::cursor::oauth::{CURSOR_CONNECT_TIMEOUT, generate_cursor_pkce, poll_cursor_login};
use crate::cursor::session::{cursor_auth_path, write_cursor_auth_file};
use crate::fetcher::Fetcher;
use crate::types::{
    CursorUsageConnect, PROVIDER_CURSOR, ProviderQuota, SOURCE_CURSOR_CONNECT, set_source,
};

impl Fetcher {
    /// Starts the browser login (or returns the one in flight). `on_done`
    /// runs when it finishes: `Ok` after tokens are written, `Err` with the
    /// context error when it timed out or was canceled.
    pub fn start_connect(
        self: &Arc<Self>,
        on_done: impl FnOnce(Result<(), CtxError>) + Send + 'static,
    ) -> CursorUsageConnect {
        let mut st = self.connect.lock();
        if st.connecting && !st.url.is_empty() {
            return CursorUsageConnect {
                login_url: st.url.clone(),
                pending: true,
                error: String::new(),
            };
        }
        let login = match generate_cursor_pkce(&self.cursor_login_url) {
            Ok(l) => l,
            Err(e) => {
                return CursorUsageConnect {
                    error: e,
                    ..Default::default()
                };
            }
        };
        let ctx = Ctx::background().with_timeout(CURSOR_CONNECT_TIMEOUT);
        st.connecting = true;
        st.cancel = Some(ctx.cancel.clone());
        st.url.clone_from(&login.login_url);
        drop(st);

        let url = login.login_url.clone();
        let f = Arc::clone(self);
        tokio::spawn(async move {
            let res = poll_cursor_login(&ctx, &f, &login).await;
            {
                let mut st = f.connect.lock();
                st.connecting = false;
                st.cancel = None;
                st.url.clear();
            }
            let mut tok = match res {
                Ok(t) => t,
                Err(e) => {
                    on_done(Err(e));
                    return;
                }
            };
            if let Some(dir) = f.data_dir() {
                let bg = Ctx::background();
                if tok.email.is_empty() {
                    tok.email = fetch_cursor_email(&bg, &f, &tok.access).await;
                }
                tok = f.ensure_cursor_api_key(&bg, tok).await;
                let _ = write_cursor_auth_file(&cursor_auth_path(&dir), &tok);
            }
            on_done(Ok(()));
        });
        CursorUsageConnect {
            login_url: url,
            pending: true,
            error: String::new(),
        }
    }

    /// Aborts an in-flight Connect.
    pub fn cancel_connect(&self) {
        let mut st = self.connect.lock();
        if let Some(c) = st.cancel.take() {
            c.cancel();
        }
        st.connecting = false;
        st.url.clear();
    }
}

pub(crate) fn cursor_hint_no_session() -> &'static str {
    "Connect in Settings."
}

/// A local-only row so Connect stays visible without a fetch.
pub fn cursor_no_session_quota() -> ProviderQuota {
    ProviderQuota {
        id: PROVIDER_CURSOR.into(),
        name: "Cursor".into(),
        error: "No Cursor usage session".into(),
        hint: cursor_hint_no_session().into(),
        ..Default::default()
    }
}

/// A local-only row so Disconnect stays reachable when Cursor is off but
/// Recur still owns Connect tokens.
pub fn cursor_connect_tokens_quota() -> ProviderQuota {
    let mut q = ProviderQuota {
        id: PROVIDER_CURSOR.into(),
        name: "Cursor".into(),
        hint: "Cursor is off in Settings — tokens kept so you can disconnect.".into(),
        ..Default::default()
    };
    set_source(&mut q, SOURCE_CURSOR_CONNECT);
    q
}

/// The in-flight Connect row (no network).
pub fn cursor_connecting_quota() -> ProviderQuota {
    ProviderQuota {
        id: PROVIDER_CURSOR.into(),
        name: "Cursor".into(),
        connecting: true,
        hint: "Waiting for browser sign-in…".into(),
        ..Default::default()
    }
}
