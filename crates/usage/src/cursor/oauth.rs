//! Recur Connect for Cursor: the browser PKCE login, session refresh, and the
//! user API key minted from the session.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Method;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::cursor_api_headers;
use super::session::{CursorTok, cursor_auth_path, write_cursor_auth_file};
use crate::ctx::{Ctx, CtxError};
use crate::fetcher::Fetcher;
use crate::http::{do_json, http_err};
use crate::parse::{
    CURSOR_REFRESH_LEEWAY_MS, as_float, as_int64, decode_map, email_from_jwt, first_any,
    first_string, slice_of,
};
use crate::types::SOURCE_CURSOR_CONNECT;

pub const DEFAULT_CURSOR_LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
pub const DEFAULT_CURSOR_POLL_URL: &str = "https://api2.cursor.sh/auth/poll";
pub const DEFAULT_CURSOR_REFRESH_URL: &str = "https://api2.cursor.sh/auth/exchange_user_api_key";
pub const DEFAULT_CURSOR_MINT_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/CreateUserApiKey";
pub const DEFAULT_CURSOR_LIST_KEYS_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/ListUserApiKeys";
pub const DEFAULT_CURSOR_REVOKE_KEY_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/RevokeUserApiKey";
pub(crate) const CURSOR_CONNECT_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const CURSOR_POLL_INTERVAL: Duration = Duration::from_millis(1500);
pub(crate) const CURSOR_MINT_KEY_TTL_MS: i64 = 90 * 24 * 3600 * 1000;
const CURSOR_MINT_KEY_NAME: &str = "Recur";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PkceLogin {
    pub verifier: String,
    pub challenge: String,
    pub uuid: String,
    pub login_url: String,
}

pub(crate) fn generate_cursor_pkce(login_base: &str) -> Result<PkceLogin, String> {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let verifier = URL_SAFE_NO_PAD.encode(raw);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let id = uuid::Uuid::now_v7().to_string();
    // redirectTarget is Cursor's portal label, not a protocol switch: `sdk`
    // reads "Sign in to an app using the Cursor SDK"; `cli` said Cursor CLI.
    // Keys sorted.
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("challenge", &challenge)
        .append_pair("mode", "login")
        .append_pair("redirectTarget", "sdk")
        .append_pair("uuid", &id)
        .finish();
    let base = if login_base.is_empty() {
        DEFAULT_CURSOR_LOGIN_URL
    } else {
        login_base
    };
    Ok(PkceLogin {
        login_url: format!("{base}?{query}"),
        verifier,
        challenge,
        uuid: id,
    })
}

fn query_escape(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Polls until the browser login completes or `ctx` ends. Transport errors
/// and pending statuses keep polling.
pub(crate) async fn poll_cursor_login(
    ctx: &Ctx,
    f: &Fetcher,
    login: &PkceLogin,
) -> Result<CursorTok, CtxError> {
    let poll_url = format!(
        "{}?uuid={}&verifier={}",
        f.url(&f.cursor_poll_url, DEFAULT_CURSOR_POLL_URL),
        query_escape(&login.uuid),
        query_escape(&login.verifier)
    );
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + CURSOR_POLL_INTERVAL,
        CURSOR_POLL_INTERVAL,
    );
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            e = ctx.done() => return Err(e),
            _ = tick.tick() => {}
        }
        let Ok((status, raw)) = do_json(
            ctx,
            &f.http,
            Method::GET,
            &poll_url,
            None,
            &[("Accept", "application/json".into())],
        )
        .await
        else {
            continue;
        };
        if matches!(status, 204 | 202 | 404 | 402) || status >= 300 {
            continue;
        }
        if let Ok(mut tok) = parse_cursor_poll(&raw)
            && !tok.access.is_empty()
        {
            tok.source = SOURCE_CURSOR_CONNECT.into();
            return Ok(tok);
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PollBody {
    #[serde(rename = "accessToken")]
    access_token_camel: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token_camel: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    email: Option<String>,
}

pub(crate) fn parse_cursor_poll(raw: &[u8]) -> Result<CursorTok, String> {
    let b = serde_json::from_slice::<Option<PollBody>>(raw)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let first = |a: Option<String>, b: Option<String>| {
        [a, b]
            .into_iter()
            .flatten()
            .map(|s| s.trim().to_owned())
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    };
    let mut tok = CursorTok {
        access: first(b.access_token_camel, b.access_token),
        refresh: first(b.refresh_token_camel, b.refresh_token),
        email: b.email.unwrap_or_default().trim().to_owned(),
        source: SOURCE_CURSOR_CONNECT.into(),
        ..Default::default()
    };
    if tok.access.is_empty() {
        return Err("poll returned no access token".into());
    }
    if tok.email.is_empty() {
        tok.email = email_from_jwt(&tok.access);
    }
    Ok(tok)
}

pub(crate) async fn refresh_cursor_token(
    ctx: &Ctx,
    f: &Fetcher,
    refresh: &str,
) -> Result<CursorTok, String> {
    if refresh.is_empty() {
        return Err("no refresh token".into());
    }
    let (status, raw) = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_refresh_url, DEFAULT_CURSOR_REFRESH_URL),
        Some(b"{}".to_vec()),
        &[
            ("Authorization", format!("Bearer {refresh}")),
            ("Content-Type", "application/json".into()),
            ("Accept", "application/json".into()),
        ],
    )
    .await?;
    if status >= 300 {
        return Err(http_err(status, &raw));
    }
    let mut tok = parse_cursor_poll(&raw)?;
    if tok.refresh.is_empty() {
        tok.refresh = refresh.to_owned();
    }
    tok.source = SOURCE_CURSOR_CONNECT.into();
    Ok(tok)
}

pub(crate) fn cursor_api_key_needs_refresh(tok: &CursorTok, now_ms: i64) -> bool {
    if tok.api_key.is_empty() {
        return true;
    }
    if tok.api_key_expires_at_ms <= 0 {
        return false;
    }
    now_ms + CURSOR_REFRESH_LEEWAY_MS >= tok.api_key_expires_at_ms
}

impl Fetcher {
    /// Mints (or rotates before expiry) the user API key the Cursor SDK uses,
    /// persists it, notifies `on_minted_api_key`, then revokes the previous
    /// key and records the new key's dashboard id.
    pub(crate) async fn ensure_cursor_api_key(&self, ctx: &Ctx, mut tok: CursorTok) -> CursorTok {
        if tok.access.is_empty() || !cursor_api_key_needs_refresh(&tok, self.now_ms()) {
            return tok;
        }
        let (prev, prev_id) = (tok.api_key.clone(), tok.api_key_id);
        let Ok((key, exp)) = mint_cursor_user_api_key(ctx, self, &tok.access).await else {
            return tok;
        };
        if key.is_empty() {
            return tok;
        }
        tok.api_key = key.clone();
        tok.api_key_expires_at_ms = exp;
        tok.api_key_id = 0;
        if let Some(dir) = self.data_dir() {
            let _ = write_cursor_auth_file(&cursor_auth_path(&dir), &tok);
        }
        if let Some(h) = &self.on_minted_api_key {
            h(&prev, &key);
        }
        if !prev.is_empty() {
            let _ = revoke_cursor_user_api_key(ctx, self, &tok.access, &prev, prev_id).await;
        }
        let id = lookup_cursor_api_key_id(ctx, self, &tok.access, &key).await;
        if id != 0 {
            tok.api_key_id = id;
            if let Some(dir) = self.data_dir() {
                let _ = write_cursor_auth_file(&cursor_auth_path(&dir), &tok);
            }
        }
        tok
    }
}

pub(crate) async fn mint_cursor_user_api_key(
    ctx: &Ctx,
    f: &Fetcher,
    access: &str,
) -> Result<(String, i64), String> {
    if access.is_empty() {
        return Err("no session token".into());
    }
    let exp = f.now_ms() + CURSOR_MINT_KEY_TTL_MS;
    let body = format!(
        "{{\"expiresAtMs\":{exp},\"name\":{}}}",
        serde_json::to_string(CURSOR_MINT_KEY_NAME).unwrap_or_default()
    );
    let (status, raw) = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_mint_url, DEFAULT_CURSOR_MINT_URL),
        Some(body.into_bytes()),
        &cursor_api_headers(access),
    )
    .await?;
    if status >= 300 {
        return Err(http_err(status, &raw));
    }
    let m = decode_map(&raw)?;
    let key = first_string(&m, &["apiKey", "api_key"]);
    if key.is_empty() {
        return Err("mint returned no api key".into());
    }
    Ok((key, exp))
}

struct ApiKeyRow {
    id: i64,
    name: String,
    masked: String,
    created: f64,
}

async fn list_cursor_user_api_keys(ctx: &Ctx, f: &Fetcher, access: &str) -> Vec<ApiKeyRow> {
    let Ok((status, raw)) = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_list_keys_url, DEFAULT_CURSOR_LIST_KEYS_URL),
        Some(b"{}".to_vec()),
        &cursor_api_headers(access),
    )
    .await
    else {
        return Vec::new();
    };
    if status >= 300 {
        return Vec::new();
    }
    let Ok(m) = decode_map(&raw) else {
        return Vec::new();
    };
    let mut rows = slice_of(m.get("apiKeys"));
    if rows.is_empty() {
        rows = slice_of(m.get("api_keys"));
    }
    rows.iter()
        .filter_map(|r| r.as_object())
        .filter_map(|rm| {
            let id = as_int64(first_any(rm, &["id"]));
            (id != 0).then(|| ApiKeyRow {
                id,
                name: first_string(rm, &["name"]),
                masked: first_string(rm, &["maskedKey", "masked_key"]),
                created: as_float(first_any(rm, &["createdAt", "created_at"])),
            })
        })
        .collect()
}

/// Whether `masked` (`crsr_...e5f6`) shows the tail of `full`.
pub(crate) fn matches_masked_cursor_key(full: &str, masked: &str) -> bool {
    let (full, masked) = (full.trim(), masked.trim());
    if full.is_empty() || masked.is_empty() {
        return false;
    }
    match masked.rfind("...") {
        None => full == masked,
        Some(i) => {
            let suffix = &masked[i + 3..];
            !suffix.is_empty() && full.ends_with(suffix)
        }
    }
}

/// The newest Recur-named dashboard key whose mask matches `full_key`.
async fn lookup_cursor_api_key_id(ctx: &Ctx, f: &Fetcher, access: &str, full_key: &str) -> i64 {
    let mut best = 0;
    let mut best_created = 0.0;
    for k in list_cursor_user_api_keys(ctx, f, access).await {
        if !k.name.is_empty() && k.name != CURSOR_MINT_KEY_NAME {
            continue;
        }
        if !matches_masked_cursor_key(full_key, &k.masked) {
            continue;
        }
        if best == 0 || k.created >= best_created {
            best = k.id;
            best_created = k.created;
        }
    }
    best
}

pub(crate) async fn revoke_cursor_user_api_key(
    ctx: &Ctx,
    f: &Fetcher,
    access: &str,
    full_key: &str,
    mut id: i64,
) -> Result<(), String> {
    if access.is_empty() {
        return Err("no session token".into());
    }
    if id == 0 {
        id = lookup_cursor_api_key_id(ctx, f, access, full_key).await;
    }
    if id == 0 {
        return Err("cursor api key id not found".into());
    }
    let (status, raw) = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_revoke_key_url, DEFAULT_CURSOR_REVOKE_KEY_URL),
        Some(format!("{{\"id\":{id}}}").into_bytes()),
        &cursor_api_headers(access),
    )
    .await?;
    if status >= 300 {
        return Err(http_err(status, &raw));
    }
    Ok(())
}
