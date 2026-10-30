//! Fetch, parse, poller and provider usage service tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::claude::*;
use crate::codex::*;
use crate::cursor::oauth::*;
use crate::cursor::session::*;
use crate::cursor::*;
use crate::*;

#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> String {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// A one-request-per-connection HTTP/1.1 server; `answer` gets the request
/// (path without query) and returns (status, body).
async fn serve(answer: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let seen = Arc::clone(&log);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let (seen, answer) = (Arc::clone(&seen), Arc::clone(&answer));
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
                    let target = lines
                        .next()
                        .and_then(|l| l.split(' ').nth(1))
                        .unwrap_or_default();
                    let path = target.split('?').next().unwrap_or_default().to_owned();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                        .collect();
                    let len: usize = headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.parse().ok())
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        let body =
                            String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                        break Seen {
                            path,
                            headers,
                            body,
                        };
                    }
                };
                seen.lock().push(req.clone());
                let (status, body) = answer(&req);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (base, log)
}

fn ok(body: &str) -> (u16, String) {
    (200, body.to_owned())
}

fn bg() -> Ctx {
    Ctx::background()
}

fn test_jwt(claims: serde_json::Value) -> String {
    format!(
        "eyJhbGciOiJub25lIn0.{}.",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

const TWO_WINDOWS: &str = r#"{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}"#;
const HOUR_MS: i64 = 3_600_000;

// ---- Claude

#[tokio::test]
async fn fetch_claude_ok() {
    let (base, seen) = serve(|_| ok(TWO_WINDOWS)).await;
    let f = Fetcher {
        claude_usage_url: base,
        claude_token: hook(|| Ok("sk-ant-test".into())),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert!(q.error.is_empty() && q.windows.len() == 2, "{q:?}");
    let s = seen.lock()[0].clone();
    assert_eq!(s.header("authorization"), "Bearer sk-ant-test");
    assert_eq!(s.header("anthropic-beta"), "oauth-2025-04-20");
}

fn creds_cell(c: ClaudeOAuth) -> Arc<Mutex<ClaudeOAuth>> {
    Arc::new(Mutex::new(c))
}

#[tokio::test]
async fn fetch_claude_nudges_expired_access_token() {
    let (base, _) = serve(|r| {
        if r.header("authorization") != "Bearer sk-ant-fresh" {
            return (401, String::new());
        }
        ok(TWO_WINDOWS)
    })
    .await;
    let now = 1_000_000;
    let creds = creds_cell(ClaudeOAuth {
        access: "sk-ant-stale".into(),
        expires_at: now - 1,
        ..Default::default()
    });
    let nudges = Arc::new(AtomicUsize::new(0));
    let (c1, c2, n) = (creds.clone(), creds.clone(), nudges.clone());
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || c1.lock().clone()),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
            *c2.lock() = ClaudeOAuth {
                access: "sk-ant-fresh".into(),
                expires_at: now + HOUR_MS,
                ..Default::default()
            };
        }),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert_eq!(nudges.load(Ordering::SeqCst), 1);
    assert!(q.error.is_empty() && q.windows.len() == 2, "{q:?}");
}

#[tokio::test]
async fn fetch_claude_retries_after_401() {
    let now = 1_000_000;
    let (base, seen) = serve(|r| {
        if r.header("authorization") != "Bearer sk-ant-fresh" {
            return (401, String::new());
        }
        ok(TWO_WINDOWS)
    })
    .await;
    let creds = creds_cell(ClaudeOAuth {
        access: "sk-ant-stale".into(),
        expires_at: now + HOUR_MS,
        ..Default::default()
    });
    let (c1, c2) = (creds.clone(), creds.clone());
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || c1.lock().clone()),
        nudge_claude: hook(move || {
            *c2.lock() = ClaudeOAuth {
                access: "sk-ant-fresh".into(),
                expires_at: now + 2 * HOUR_MS,
                ..Default::default()
            };
        }),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert!(q.error.is_empty() && q.windows.len() == 2, "{q:?}");
    let auths: Vec<String> = seen
        .lock()
        .iter()
        .map(|s| s.header("authorization"))
        .collect();
    assert_eq!(auths, ["Bearer sk-ant-stale", "Bearer sk-ant-fresh"]);
}

#[tokio::test]
async fn fetch_claude_does_not_nudge_when_logged_out() {
    let nudges = Arc::new(AtomicUsize::new(0));
    let n = nudges.clone();
    let f = Fetcher {
        claude_creds: hook(ClaudeOAuth::default),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
        }),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert_eq!(q.error, "Not signed in to Claude Code");
    assert_eq!(nudges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fetch_claude_token_override_skips_nudge() {
    let (base, _) = serve(|_| (401, String::new())).await;
    let nudges = Arc::new(AtomicUsize::new(0));
    let n = nudges.clone();
    let f = Fetcher {
        claude_usage_url: base,
        claude_token: hook(|| Ok("tok".into())),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
        }),
        ..Fetcher::default()
    };
    assert_eq!(
        fetch_claude(&bg(), &f).await.error,
        "Claude Code login expired"
    );
    assert_eq!(nudges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fetch_claude_does_not_nudge_twice() {
    let now = 1_000_000;
    let (base, _) = serve(|_| (401, String::new())).await;
    let nudges = Arc::new(AtomicUsize::new(0));
    let n = nudges.clone();
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || ClaudeOAuth {
            access: "sk-ant-stale".into(),
            expires_at: now - 1,
            ..Default::default()
        }),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
        }),
        ..Fetcher::default()
    };
    assert_eq!(
        fetch_claude(&bg(), &f).await.error,
        "Claude Code login expired"
    );
    assert_eq!(
        nudges.load(Ordering::SeqCst),
        1,
        "expiry nudge, no 401 retry"
    );
}

#[tokio::test]
async fn fetch_claude_does_not_retry_403() {
    let now = 1_000_000;
    let (base, _) = serve(|_| (403, String::new())).await;
    let nudges = Arc::new(AtomicUsize::new(0));
    let n = nudges.clone();
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || ClaudeOAuth {
            access: "tok".into(),
            expires_at: now + HOUR_MS,
            ..Default::default()
        }),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
        }),
        ..Fetcher::default()
    };
    assert_eq!(
        fetch_claude(&bg(), &f).await.error,
        "Claude Code login expired"
    );
    assert_eq!(nudges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn claude_step_ctx_reserves_http() {
    let parent = bg().with_timeout(Duration::from_secs(12));
    let step = claude_step_ctx(&parent, Duration::from_secs(5), Duration::from_secs(4));
    let left = step.remaining().unwrap();
    assert!(left <= Duration::from_millis(5050), "{left:?}");
    assert!(parent.remaining().unwrap() - left >= Duration::from_secs(3));
    let tight = bg().with_timeout(Duration::from_secs(3));
    let step = claude_step_ctx(&tight, Duration::from_secs(5), Duration::from_secs(4));
    assert!(step.remaining().unwrap() <= Duration::from_secs(1));
}

#[tokio::test]
async fn fetch_claude_nudges_refresh_only_blob() {
    let now = 1_000_000;
    let (base, _) = serve(|r| {
        if r.header("authorization") != "Bearer sk-ant-fresh" {
            return (401, String::new());
        }
        ok(TWO_WINDOWS)
    })
    .await;
    let creds = creds_cell(ClaudeOAuth {
        refresh: "r".into(),
        ..Default::default()
    });
    let nudges = Arc::new(AtomicUsize::new(0));
    let (c1, c2, n) = (creds.clone(), creds.clone(), nudges.clone());
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || c1.lock().clone()),
        nudge_claude: hook(move || {
            n.fetch_add(1, Ordering::SeqCst);
            *c2.lock() = ClaudeOAuth {
                access: "sk-ant-fresh".into(),
                refresh: "r".into(),
                expires_at: now + HOUR_MS,
            };
        }),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert_eq!(nudges.load(Ordering::SeqCst), 1);
    assert!(q.error.is_empty() && q.windows.len() == 2, "{q:?}");
}

#[tokio::test]
async fn fetch_claude_keeps_creds_when_reread_empty() {
    let now = 1_000_000;
    let (base, _) = serve(|r| {
        if r.header("authorization") != "Bearer sk-ant-stale" {
            return (401, String::new());
        }
        ok(TWO_WINDOWS)
    })
    .await;
    let n = Arc::new(AtomicUsize::new(0));
    let f = Fetcher {
        claude_usage_url: base,
        now: hook(move || now),
        claude_creds: hook(move || {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                ClaudeOAuth {
                    access: "sk-ant-stale".into(),
                    expires_at: now - 1,
                    ..Default::default()
                }
            } else {
                ClaudeOAuth::default()
            }
        }),
        nudge_claude: hook(|| {}),
        ..Fetcher::default()
    };
    let q = fetch_claude(&bg(), &f).await;
    assert!(q.error.is_empty() && q.windows.len() == 2, "{q:?}");
}

#[tokio::test]
async fn claude_fetch_ctx_caps_when_others_wanted() {
    let parent = bg().with_timeout(poller::COLLECT_TIMEOUT);
    let capped = crate::fetcher::claude_fetch_ctx(&parent, true);
    assert!(capped.deadline < parent.deadline);
    let solo = crate::fetcher::claude_fetch_ctx(&parent, false);
    assert_eq!(solo.deadline, parent.deadline);
}

#[test]
fn parse_claude_credentials() {
    for (raw, want) in [
        ("sk-ant-oat01-abc", "sk-ant-oat01-abc"),
        (
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xyz","refreshToken":"r"}}"#,
            "sk-ant-oat01-xyz",
        ),
        (r#"{"access_token":"sk-ant-oat01-sn"}"#, "sk-ant-oat01-sn"),
        ("not-json", ""),
        ("{}", ""),
    ] {
        assert_eq!(parse_claude_oauth(raw.as_bytes()).access, want, "{raw}");
    }
    let got = parse_claude_oauth(br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xyz","refreshToken":"sk-ant-ort01-r","expiresAt":1787565508430}}"#);
    assert_eq!(
        (got.refresh.as_str(), got.expires_at),
        ("sk-ant-ort01-r", 1_787_565_508_430)
    );
}

#[test]
fn pick_claude_creds_cases() {
    let c = |access: &str, refresh: &str, exp: i64| ClaudeOAuth {
        access: access.into(),
        refresh: refresh.into(),
        expires_at: exp,
    };
    for (name, kc, file, want) in [
        ("file newer", c("kc", "", 1000), c("file", "", 2000), "file"),
        (
            "keychain newer",
            c("kc", "", 2000),
            c("file", "", 1000),
            "kc",
        ),
        (
            "unknown expiry prefers keychain",
            c("kc", "", 0),
            c("file", "", 0),
            "kc",
        ),
        (
            "file known beats unknown keychain",
            c("kc", "", 0),
            c("file", "", 2000),
            "file",
        ),
        (
            "file only",
            ClaudeOAuth::default(),
            c("file", "", 2000),
            "file",
        ),
        (
            "keychain only",
            c("kc", "", 1000),
            ClaudeOAuth::default(),
            "kc",
        ),
        (
            "refresh-only file",
            ClaudeOAuth::default(),
            c("", "r", 0),
            "r",
        ),
        ("empty", ClaudeOAuth::default(), ClaudeOAuth::default(), ""),
    ] {
        let got = pick_claude_creds(kc, file);
        let have = if got.access.is_empty() {
            got.refresh
        } else {
            got.access
        };
        assert_eq!(have, want, "{name}");
    }
}

#[test]
fn claude_access_needs_refresh_cases() {
    let now = 1_000_000;
    let c = |access: &str, refresh: &str, exp: i64| ClaudeOAuth {
        access: access.into(),
        refresh: refresh.into(),
        expires_at: exp,
    };
    for (name, creds, want) in [
        ("missing access", c("", "", 1), false),
        ("unknown expiry", c("tok", "", 0), false),
        ("expired", c("tok", "", now - 1), true),
        ("inside leeway", c("tok", "", now + 60_000), true),
        ("fresh", c("tok", "", now + HOUR_MS), false),
        ("refresh only", c("", "r", 0), true),
    ] {
        assert_eq!(claude_access_needs_refresh(&creds, now), want, "{name}");
    }
}

#[test]
fn parse_claude_usage_cases() {
    let q = parse_claude_usage(
        br#"{"five_hour":{"utilization":78.5,"resets_at":"2026-08-21T01:00:00Z"},"seven_day":{"used_percentage":13,"resets_at":1775808000},"seven_day_opus":{"used_percent":4.2},"extra_usage":{"is_enabled":true,"used_credits":10000,"monthly_limit":10000},"limits":[{"kind":"weekly_scoped","percent":54,"resets_at":"2026-08-28T00:00:00Z","scope":{"model":{"display_name":"Fable"}}}]}"#,
    )
    .unwrap();
    assert_eq!(q.id, PROVIDER_CLAUDE);
    assert_eq!(q.windows[0].used_pct, 78.5);
    assert_eq!(q.windows[0].resets_at, 1_787_274_000_000);
    assert_eq!(q.windows[1].used_pct, 13.0);
    let fable = q.windows.iter().find(|w| w.id == WINDOW_FABLE).unwrap();
    assert_eq!(
        (fable.used_pct, fable.label.as_str()),
        (54.0, "Fable (7-day)")
    );
    let extra = q.windows.iter().find(|w| w.id == WINDOW_EXTRA).unwrap();
    assert_eq!(
        (extra.used_pct, extra.detail.as_str()),
        (100.0, "$100.00 of $100.00")
    );
    assert!(parse_claude_usage(br#"{"email":"a@b.c"}"#).is_err());
    assert_eq!(
        parse_claude_account_file(
            br#"{"oauthAccount":{"emailAddress":"dev@example.com","organizationName":"Acme"}}"#
        ),
        "dev@example.com"
    );
}

#[test]
fn claude_credential_paths_honors_config_dir() {
    let paths = claude_credential_paths_in("/cfg", Some("/home/u"));
    assert_eq!(paths[0], "/cfg/.credentials.json");
    assert_eq!(
        paths.last().map(String::as_str),
        Some("/home/u/.claude.json")
    );
    assert_eq!(
        claude_credential_paths_in("", Some("/home/u"))[0],
        "/home/u/.claude/.credentials.json"
    );
}

fn claude_window<'a>(q: &'a ProviderQuota, id: &str) -> &'a Window {
    q.windows
        .iter()
        .find(|w| w.id == id)
        .unwrap_or_else(|| panic!("no window {id} in {:?}", q.windows))
}

#[test]
fn parse_claude_usage_keeps_dotless_versioned_scoped_windows() {
    let q = parse_claude_usage(
        br#"{"five_hour":{"utilization":1},"seven_day":{"utilization":2},"limits":[
            {"kind":"weekly_scoped","percent":80,"is_active":false,"scope":{"model":{"display_name":"Opus 4"}}},
            {"kind":"weekly_scoped","percent":10,"is_active":true,"scope":{"model":{"display_name":"Opus 5"}}}]}"#,
    )
    .unwrap();
    let older = claude_window(&q, "seven_day_opus_4");
    assert_eq!(
        (older.used_pct, older.label.as_str()),
        (80.0, "Opus 4 (7-day)")
    );
    let newer = claude_window(&q, "seven_day_opus_5");
    assert_eq!(
        (newer.used_pct, newer.label.as_str()),
        (10.0, "Opus 5 (7-day)")
    );
}

#[test]
fn parse_claude_usage_prefers_active_scoped_limit() {
    let q = parse_claude_usage(
        br#"{"five_hour":{"utilization":1},"seven_day":{"utilization":2},"limits":[
            {"kind":"weekly_scoped","percent":9,"is_active":false,"scope":{"model":{"display_name":"Fable"}}},
            {"kind":"weekly_scoped","percent":44,"is_active":true,"scope":{"model":{"display_name":"Fable"}}}]}"#,
    )
    .unwrap();
    assert_eq!(claude_window(&q, WINDOW_FABLE).used_pct, 44.0);
}

#[test]
fn parse_claude_usage_labels_fable_slug() {
    for (slug, id, label) in [
        ("claude-fable-5", "seven_day_fable_5", "Fable 5 (7-day)"),
        (
            "claude-fable-5-1",
            "seven_day_fable_5_1",
            "Fable 5.1 (7-day)",
        ),
    ] {
        let raw = format!(
            r#"{{"five_hour":{{"utilization":8}},"seven_day":{{"utilization":2}},"model_scoped":[{{"name":"{slug}","utilization":40}}]}}"#
        );
        let q = parse_claude_usage(raw.as_bytes()).unwrap();
        let w = claude_window(&q, id);
        assert_eq!((w.used_pct, w.label.as_str()), (40.0, label), "{slug}");
    }
}

// ---- Codex

#[tokio::test]
async fn fetch_codex_api_key_only() {
    let f = Fetcher {
        codex_auth: hook(|| {
            Ok(CodexAuth {
                has_api_key: true,
                ..Default::default()
            })
        }),
        ..Fetcher::default()
    };
    let q = fetch_codex(&bg(), &f).await;
    assert_eq!(q.source, SOURCE_CODEX_API_KEY);
    assert!(q.error.contains("no plan quota"), "{}", q.error);
}

#[tokio::test]
async fn fetch_codex_ok() {
    let (base, seen) =
        serve(|_| ok(r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":5}}}"#))
            .await;
    let f = Fetcher {
        codex_usage_url: base,
        codex_auth: hook(|| {
            Ok(CodexAuth {
                access_token: "chat-tok".into(),
                account_id: "acc".into(),
                email: "codex@example.com".into(),
                has_chatgpt: true,
                ..Default::default()
            })
        }),
        ..Fetcher::default()
    };
    let q = fetch_codex(&bg(), &f).await;
    assert!(
        q.error.is_empty()
            && q.plan == "Plus"
            && q.email == "codex@example.com"
            && q.windows.len() == 1,
        "{q:?}"
    );
    let seen = seen.lock();
    assert_eq!(seen.len(), 1, "no monthly fallback without spend_control");
    assert_eq!(seen[0].header("authorization"), "Bearer chat-tok");
    assert_eq!(seen[0].header("chatgpt-account-id"), "acc");
}

async fn codex_monthly(usage: &'static str, auth: CodexAuth) -> (ProviderQuota, Log) {
    let (base, seen) = serve(move |r| match r.path.as_str() {
        "/wham/usage" => ok(usage),
        "/accounts/acc/spend-controls/current-user/monthly-usage" => {
            ok(r#"{"current_month_usage":297.4,"effective_monthly_limit":{"limit":1000,"enforcement_mode":"HARD_CAP"}}"#)
        }
        _ => (404, String::new()),
    })
    .await;
    let f = Fetcher {
        codex_usage_url: format!("{base}/wham/usage"),
        codex_auth: hook(move || Ok(auth.clone())),
        ..Fetcher::default()
    };
    (fetch_codex(&bg(), &f).await, seen)
}

#[tokio::test]
async fn fetch_codex_monthly_fallback() {
    let (q, seen) = codex_monthly(
        r#"{"plan_type":"education","rate_limit":{"primary_window":{"used_percent":14},"secondary_window":{"used_percent":33}},"spend_control":{"reached":false,"individual_limit":null}}"#,
        CodexAuth { access_token: "chat-tok".into(), account_id: "acc".into(), has_chatgpt: true, ..Default::default() },
    )
    .await;
    assert!(q.error.is_empty(), "{q:?}");
    let m = q.windows.iter().find(|w| w.id == WINDOW_BILLING).unwrap();
    assert_eq!(
        (m.used_pct, m.detail.as_str()),
        (29.74, "297 of 1,000 credits")
    );
    let monthly = seen.lock()[1].clone();
    assert_eq!(monthly.header("authorization"), "Bearer chat-tok");
    assert_eq!(monthly.header("chatgpt-account-id"), "acc");
}

#[tokio::test]
async fn fetch_codex_monthly_fallback_uses_usage_account_id() {
    let (q, seen) = codex_monthly(
        r#"{"plan_type":"education","account_id":"acc","rate_limit":{"primary_window":{"used_percent":14}},"spend_control":{"reached":false,"individual_limit":null}}"#,
        CodexAuth { access_token: "chat-tok".into(), has_chatgpt: true, ..Default::default() },
    )
    .await;
    assert!(q.windows.iter().any(|w| w.id == WINDOW_BILLING), "{q:?}");
    assert_eq!(seen.lock()[1].header("chatgpt-account-id"), "acc");
}

#[test]
fn resolve_codex_account_id_cases() {
    let auth = |a: &str, at: &str, it: &str| CodexAuth {
        account_id: a.into(),
        access_token: at.into(),
        id_token: it.into(),
        ..Default::default()
    };
    assert_eq!(
        resolve_codex_account_id(
            &auth("from-auth", "", ""),
            br#"{"account_id":"from-usage"}"#
        ),
        "from-auth"
    );
    assert_eq!(
        resolve_codex_account_id(&auth("", "", ""), br#"{"account_id":"from-usage"}"#),
        "from-usage"
    );
    let tok = test_jwt(
        serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": "from-jwt"}}),
    );
    assert_eq!(
        resolve_codex_account_id(&auth("", &tok, ""), b"{}"),
        "from-jwt"
    );
    let id = test_jwt(
        serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": "from-id-token"}}),
    );
    assert_eq!(
        resolve_codex_account_id(&auth("", "", &id), b"{}"),
        "from-id-token"
    );
}

#[test]
fn codex_monthly_url() {
    assert_eq!(
        codex_monthly_usage_url(DEFAULT_CODEX_USAGE_URL, "acc"),
        "https://chatgpt.com/backend-api/accounts/acc/spend-controls/current-user/monthly-usage"
    );
    assert_eq!(codex_monthly_usage_url(DEFAULT_CODEX_USAGE_URL, ""), "");
}

#[test]
fn parse_codex_auth_cases() {
    let a = parse_codex_auth(br#"{"tokens":{"access_token":"tok","account_id":"acc-1"}}"#).unwrap();
    assert!(a.access_token == "tok" && a.account_id == "acc-1" && a.has_chatgpt);
    let id = test_jwt(serde_json::json!({"email": "codex@example.com"}));
    let raw = format!(r#"{{"tokens":{{"access_token":"tok","id_token":"{id}"}}}}"#);
    assert_eq!(
        parse_codex_auth(raw.as_bytes()).unwrap().email,
        "codex@example.com"
    );
    let key = parse_codex_auth(br#"{"OPENAI_API_KEY":"sk-x"}"#).unwrap();
    assert!(key.has_api_key && !key.has_chatgpt);
    assert!(parse_codex_auth(b"{}").is_err());
}

#[test]
fn credit_qty() {
    for (n, want) in [
        (0.0, "0"),
        (393.0, "393"),
        (1000.0, "1,000"),
        (297.4, "297"),
        (150_000.0, "150,000"),
    ] {
        assert_eq!(format_codex_credit_qty(n), want);
    }
    assert_eq!(format_codex_credits(393.0, 1000.0), "393 of 1,000 credits");
}

// ---- Cursor

#[tokio::test]
async fn fetch_cursor_period_and_models() {
    let (base, seen) = serve(|r| match r.path.as_str() {
        "/period" => ok(r#"{"billingCycleStart":"2026-08-01T00:00:00Z","billingCycleEnd":"2026-09-01T00:00:00Z","planUsage":{"autoPercentUsed":40,"apiPercentUsed":0,"totalPercentUsed":21.9,"limit":2000,"totalSpend":800}}"#),
        "/agg" => ok(r#"{"aggregations":[{"modelIntent":"composer-2.5","inputTokens":100,"outputTokens":20,"totalCents":80,"tier":2}]}"#),
        "/me" => ok(r#"{"user":{"email":"connect@cursor.dev"}}"#),
        _ => (404, String::new()),
    })
    .await;
    let f = Fetcher {
        cursor_period_url: format!("{base}/period"),
        cursor_agg_url: format!("{base}/agg"),
        cursor_profile_url: format!("{base}/missing"),
        cursor_me_url: format!("{base}/me"),
        // The key exists and is fresh, so no mint call.
        cursor_token: hook(|| {
            Ok(CursorTok {
                access: "connect-sess".into(),
                api_key: "crsr_x".into(),
                source: SOURCE_CURSOR_CONNECT.into(),
                ..Default::default()
            })
        }),
        ..Fetcher::default()
    };
    let q = fetch_cursor(&bg(), &f).await;
    assert!(q.error.is_empty(), "{q:?}");
    assert_eq!(
        (q.email.as_str(), q.source.as_str()),
        ("connect@cursor.dev", SOURCE_CURSOR_CONNECT)
    );
    assert_eq!(q.windows.len(), 1);
    assert_eq!(
        (q.windows[0].used_pct, q.windows[0].detail.as_str()),
        (40.0, "")
    );
    assert_eq!(q.models.len(), 1);
    assert_eq!(
        (
            q.models[0].tokens,
            q.models[0].used_pct,
            q.models[0].bucket.as_str()
        ),
        (120, 40.0, BUCKET_CURSOR)
    );
    let period = seen.lock()[0].clone();
    assert_eq!(period.header("authorization"), "Bearer connect-sess");
    let agg = seen
        .lock()
        .iter()
        .find(|s| s.path == "/agg")
        .cloned()
        .unwrap();
    assert_eq!(
        agg.body,
        r#"{"endDate":"2026-09-01T00:00:00Z","startDate":"2026-08-01T00:00:00Z"}"#
    );
}

#[tokio::test]
async fn fetch_cursor_email_from_dashboard_me() {
    let (base, seen) = serve(|_| ok(r#"{"email":"connect@cursor.dev","sub":"user_01abc"}"#)).await;
    let tok = test_jwt(serde_json::json!({"sub": "user_01abc", "type": "session"}));
    let f = Fetcher {
        cursor_web_me_url: format!("{base}/webme"),
        ..Fetcher::default()
    };
    assert_eq!(
        fetch_cursor_email(&bg(), &f, &tok).await,
        "connect@cursor.dev"
    );
    assert!(
        seen.lock()[0]
            .header("cookie")
            .contains("WorkosCursorSessionToken=user_01abc::")
    );
}

#[tokio::test]
async fn api_key_is_not_cursor_usage_token() {
    let f = Fetcher {
        want_cursor: true,
        cursor_token: hook(|| Ok(CursorTok::default())),
        ..Fetcher::default()
    };
    let q = fetch_cursor(&bg(), &f).await;
    assert!(!q.error.is_empty());
    assert!(q.hint.contains("Connect in Settings"), "{}", q.hint);
}

#[tokio::test]
async fn mint_cursor_user_api_key_body_and_expiry() {
    let (base, seen) = serve(|_| ok(r#"{"apiKey":"crsr_minted"}"#)).await;
    let now = 1_787_313_600_000;
    let f = Fetcher {
        cursor_mint_url: base,
        now: hook(move || now),
        ..Fetcher::default()
    };
    let (key, exp) = mint_cursor_user_api_key(&bg(), &f, "sess").await.unwrap();
    assert_eq!(key, "crsr_minted");
    assert_eq!(exp, now + CURSOR_MINT_KEY_TTL_MS);
    let s = seen.lock()[0].clone();
    assert_eq!(s.header("authorization"), "Bearer sess");
    assert!(s.body.contains(r#""name":"Recur""#), "{}", s.body);
}

#[tokio::test]
async fn ensure_cursor_api_key_mints_once() {
    let mints = Arc::new(AtomicUsize::new(0));
    let m = mints.clone();
    let (base, _) = serve(move |r| match r.path.as_str() {
        "/mint" => {
            m.fetch_add(1, Ordering::SeqCst);
            ok(r#"{"apiKey":"crsr_once"}"#)
        }
        _ => ok(r#"{"apiKeys":[]}"#),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let notified = Arc::new(Mutex::new(String::new()));
    let n = notified.clone();
    let f = Fetcher {
        cursor_mint_url: format!("{base}/mint"),
        cursor_list_keys_url: format!("{base}/list"),
        data_dir: hook(move || Some(path.clone())),
        on_minted_api_key: Some(Arc::new(move |_: &str, next: &str| *n.lock() = next.into())),
        ..Fetcher::default()
    };
    let tok = f
        .ensure_cursor_api_key(
            &bg(),
            CursorTok {
                access: "sess".into(),
                source: SOURCE_CURSOR_CONNECT.into(),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(tok.api_key, "crsr_once");
    assert_eq!(*notified.lock(), "crsr_once");
    assert_eq!(recur_cursor_api_key(dir.path()), "crsr_once");
    f.ensure_cursor_api_key(&bg(), tok).await;
    assert_eq!(mints.load(Ordering::SeqCst), 1);
}

#[test]
fn cursor_api_key_needs_refresh_cases() {
    let now = 1_787_313_600_000;
    let day = 24 * HOUR_MS;
    let k = |exp: i64| CursorTok {
        api_key: "crsr_x".into(),
        api_key_expires_at_ms: exp,
        ..Default::default()
    };
    assert!(cursor_api_key_needs_refresh(&CursorTok::default(), now));
    assert!(!cursor_api_key_needs_refresh(&k(0), now));
    assert!(cursor_api_key_needs_refresh(&k(now + 6 * day), now));
    assert!(!cursor_api_key_needs_refresh(&k(now + 8 * day), now));
}

#[tokio::test]
async fn ensure_cursor_api_key_rotates_and_revokes() {
    let (minted, revoked) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (m, r) = (minted.clone(), revoked.clone());
    let (base, seen) = serve(move |req| match req.path.as_str() {
        "/mint" => {
            m.fetch_add(1, Ordering::SeqCst);
            ok(r#"{"apiKey":"crsr_newkeyXXXX"}"#)
        }
        "/list" => ok(r#"{"apiKeys":[{"id":11,"name":"Recur","maskedKey":"crsr_...old1","createdAt":"1"},{"id":22,"name":"Recur","maskedKey":"crsr_...XXXX","createdAt":"2"}]}"#),
        "/revoke" => {
            r.fetch_add(1, Ordering::SeqCst);
            ok("{}")
        }
        _ => (404, String::new()),
    })
    .await;
    let now = 1_787_313_600_000;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let pair = Arc::new(Mutex::new((String::new(), String::new())));
    let p = pair.clone();
    let f = Fetcher {
        cursor_mint_url: format!("{base}/mint"),
        cursor_list_keys_url: format!("{base}/list"),
        cursor_revoke_key_url: format!("{base}/revoke"),
        data_dir: hook(move || Some(path.clone())),
        now: hook(move || now),
        on_minted_api_key: Some(Arc::new(move |prev: &str, next: &str| {
            *p.lock() = (prev.into(), next.into())
        })),
        ..Fetcher::default()
    };
    let tok = CursorTok {
        access: "sess".into(),
        api_key: "crsr_oldkeyold1".into(),
        api_key_id: 11,
        api_key_expires_at_ms: now + HOUR_MS,
        source: SOURCE_CURSOR_CONNECT.into(),
        ..Default::default()
    };
    let got = f.ensure_cursor_api_key(&bg(), tok).await;
    assert_eq!(got.api_key, "crsr_newkeyXXXX");
    assert_eq!(
        *pair.lock(),
        ("crsr_oldkeyold1".into(), "crsr_newkeyXXXX".into())
    );
    assert_eq!(
        (
            minted.load(Ordering::SeqCst),
            revoked.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    assert_eq!(got.api_key_id, 22);
    let revoke = seen
        .lock()
        .iter()
        .find(|s| s.path == "/revoke")
        .cloned()
        .unwrap();
    assert!(revoke.body.contains(r#""id":11"#), "{}", revoke.body);
    f.ensure_cursor_api_key(&bg(), got).await;
    assert_eq!(minted.load(Ordering::SeqCst), 1);
}

#[test]
fn masked_cursor_keys() {
    assert!(matches_masked_cursor_key("crsr_abcdefe5f6", "crsr_...e5f6"));
    assert!(!matches_masked_cursor_key(
        "crsr_abcdefe5f6",
        "crsr_...zzzz"
    ));
}

#[test]
fn generate_pkce_is_unique() {
    let a = generate_cursor_pkce("https://cursor.com/loginDeepControl").unwrap();
    let b = generate_cursor_pkce("https://cursor.com/loginDeepControl").unwrap();
    assert!(a.verifier != b.verifier && a.uuid != b.uuid);
    assert!(
        a.login_url.contains("redirectTarget=sdk") && a.login_url.contains("challenge="),
        "{}",
        a.login_url
    );
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        <sha2::Sha256 as sha2::Digest>::digest(a.verifier.as_bytes()),
    );
    assert_eq!(a.challenge, challenge);
}

#[tokio::test]
async fn connect_polls_until_tokens_then_writes_them() {
    let polls = Arc::new(AtomicUsize::new(0));
    let p = polls.clone();
    let (base, seen) = serve(move |r| match r.path.as_str() {
        "/poll" => {
            if p.fetch_add(1, Ordering::SeqCst) == 0 {
                (404, String::new())
            } else {
                ok(r#"{"accessToken":"sess","refreshToken":"r","email":"me@x.io"}"#)
            }
        }
        "/mint" => ok(r#"{"apiKey":"crsr_new"}"#),
        _ => ok(r#"{"apiKeys":[]}"#),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let f = Arc::new(Fetcher {
        cursor_poll_url: format!("{base}/poll"),
        cursor_mint_url: format!("{base}/mint"),
        cursor_list_keys_url: format!("{base}/list"),
        data_dir: hook(move || Some(path.clone())),
        ..Fetcher::default()
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let conn = f.start_connect(move |res| {
        let _ = tx.send(res);
    });
    assert!(
        conn.pending
            && conn
                .login_url
                .starts_with("https://cursor.com/loginDeepControl?challenge="),
        "{conn:?}"
    );
    assert!(f.cursor_connecting());
    let again = f.start_connect(|_| {});
    assert_eq!(
        again.login_url, conn.login_url,
        "a second Connect joins the first"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .unwrap()
            .unwrap(),
        Ok(())
    );
    assert!(!f.cursor_connecting());
    let tok = read_cursor_auth_file(&cursor_auth_path(dir.path())).unwrap();
    assert_eq!(
        (
            tok.access.as_str(),
            tok.email.as_str(),
            tok.api_key.as_str()
        ),
        ("sess", "me@x.io", "crsr_new")
    );
    let poll = seen.lock()[0].clone();
    assert_eq!(poll.path, "/poll");
}

#[tokio::test]
async fn cancel_connect_ends_the_login() {
    let f = Arc::new(Fetcher {
        cursor_poll_url: "http://127.0.0.1:9/poll".into(),
        ..Fetcher::default()
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    f.start_connect(move |res| {
        let _ = tx.send(res);
    });
    f.cancel_connect();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap(),
        Err(CtxError::Canceled)
    );
    assert!(!f.cursor_connecting());
}

#[test]
fn cursor_auth_files() {
    let tok = parse_cursor_auth_file(
        br#"{"accessToken":"sess","refreshToken":"r","email":"a@b.c","apiKey":"crsr_x"}"#,
    )
    .unwrap();
    assert!(tok.access == "sess" && tok.email == "a@b.c" && tok.api_key == "crsr_x");
    assert!(parse_cursor_auth_file(b"{}").is_err());

    let dir = tempfile::tempdir().unwrap();
    assert!(default_cursor_token(Some(dir.path())).is_err());
    assert!(!has_recur_cursor_auth_file(dir.path()));
    let tok = CursorTok {
        access: "recur-sess".into(),
        email: "recur@x.com".into(),
        source: SOURCE_CURSOR_CONNECT.into(),
        ..Default::default()
    };
    write_cursor_auth_file(&cursor_auth_path(dir.path()), &tok).unwrap();
    let got = default_cursor_token(Some(dir.path())).unwrap();
    assert_eq!(
        (got.access.as_str(), got.source.as_str()),
        ("recur-sess", SOURCE_CURSOR_CONNECT)
    );
    assert!(has_recur_cursor_auth_file(dir.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(cursor_auth_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    delete_recur_cursor_auth(dir.path()).unwrap();
    assert!(!has_recur_cursor_auth_file(dir.path()));
}

#[test]
fn jwt_needs_refresh_cases() {
    let jwt = |exp: i64| {
        format!(
            "x.{}.y",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#))
        )
    };
    let now = chrono::Utc::now().timestamp();
    let day = 24 * 3600;
    assert!(!crate::parse::jwt_needs_refresh("not-a-jwt"));
    assert!(!crate::parse::jwt_needs_refresh(&jwt(4_102_444_800)));
    assert!(crate::parse::jwt_needs_refresh(&jwt(1)));
    assert!(crate::parse::jwt_needs_refresh(&jwt(now + 6 * day)));
    assert!(!crate::parse::jwt_needs_refresh(&jwt(now + 8 * day)));
}

#[test]
fn used_pct_from_missing() {
    let m = |v: serde_json::Value| v.as_object().cloned().unwrap();
    assert_eq!(crate::parse::used_pct_from(None), None);
    assert_eq!(
        crate::parse::used_pct_from(Some(&m(serde_json::json!({"resets_at": 1})))),
        None
    );
    assert_eq!(
        crate::parse::used_pct_from(Some(&m(serde_json::json!({"utilization": 0})))),
        Some(0.0)
    );
}

// ---- Collect and poller

#[tokio::test]
async fn collect_honors_want_callback() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (c1, c2, c3) = (calls.clone(), calls.clone(), calls.clone());
    let f = Fetcher {
        want_claude: true,
        want_codex: true,
        want_cursor: true,
        want: hook(|| (false, false, false)),
        claude_token: hook(move || {
            c1.fetch_add(1, Ordering::SeqCst);
            Err("no".into())
        }),
        codex_auth: hook(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            Err("no".into())
        }),
        cursor_token: hook(move || {
            c3.fetch_add(1, Ordering::SeqCst);
            Err("no".into())
        }),
        ..Fetcher::default()
    };
    let snap = f.collect(&bg()).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(snap.providers, Some(vec![]));
}

#[tokio::test]
async fn collect_keeps_connecting_over_stub() {
    let f = Fetcher {
        want: hook(|| (false, false, false)),
        cursor_stub: hook(|| Some(cursor_no_session_quota())),
        ..Fetcher::default()
    };
    f.connect.lock().connecting = true;
    let snap = f.collect(&bg()).await;
    let providers = snap.providers.unwrap();
    assert!(
        providers.len() == 1 && providers[0].connecting && providers[0].error.is_empty(),
        "{providers:?}"
    );
}

#[tokio::test]
async fn collect_cursor_stub_without_fetch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let f = Fetcher {
        want_cursor: true,
        want: hook(|| (false, false, false)),
        cursor_stub: hook(|| Some(cursor_connect_tokens_quota())),
        cursor_token: hook(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(CursorTok {
                access: "x".into(),
                ..Default::default()
            })
        }),
        ..Fetcher::default()
    };
    let providers = f.collect(&bg()).await.providers.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        providers.len() == 1 && providers[0].source_id == SOURCE_ID_CURSOR_CONNECT,
        "{providers:?}"
    );
}

#[tokio::test]
async fn poller_refresh_and_stale() {
    let n = Arc::new(AtomicUsize::new(0));
    let (base, _) = serve(move |_| {
        if n.fetch_add(1, Ordering::SeqCst) == 0 {
            ok(r#"{"five_hour":{"utilization":11}}"#)
        } else {
            (429, "rate_limit_error".into())
        }
    })
    .await;
    let now = Arc::new(Mutex::new(1_787_227_200_000i64));
    let clock = now.clone();
    let f = Fetcher {
        claude_usage_url: base,
        claude_token: hook(|| Ok("tok".into())),
        want_claude: true,
        now: hook(move || *clock.lock()),
        ..Fetcher::default()
    };
    let published = Arc::new(AtomicUsize::new(0));
    let pubs = published.clone();
    let p = Poller::new(
        Arc::new(f),
        Some(Arc::new(move |name: &'static str, _snap: Snapshot| {
            assert_eq!(name, "provider.usage");
            pubs.fetch_add(1, Ordering::SeqCst);
        })),
    );
    let first = p.refresh().await;
    assert!(!first.stale);
    assert_eq!(
        first.providers.as_ref().unwrap()[0].windows[0].used_pct,
        11.0
    );
    *now.lock() += 60_000;
    let second = p.refresh().await;
    assert!(second.stale);
    assert_eq!(
        second.providers.as_ref().unwrap()[0].windows[0].used_pct,
        11.0
    );
    assert_eq!(published.load(Ordering::SeqCst), 2);
    p.stop();
}

#[tokio::test]
async fn poller_snapshot_uses_freshness_window() {
    let collects = Arc::new(AtomicUsize::new(0));
    let c = collects.clone();
    let now = Arc::new(Mutex::new(1_000_000i64));
    let clock = now.clone();
    let f = Fetcher {
        want_claude: true,
        now: hook(move || *clock.lock()),
        claude_token: hook(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Err("no".into())
        }),
        ..Fetcher::default()
    };
    let p = Poller::new(Arc::new(f), None);
    p.snapshot(false).await;
    p.snapshot(false).await;
    assert_eq!(
        collects.load(Ordering::SeqCst),
        1,
        "fresh snapshot served from cache"
    );
    p.snapshot(true).await;
    assert_eq!(
        collects.load(Ordering::SeqCst),
        2,
        "force bypasses the cache"
    );
    *now.lock() += 61_000;
    p.snapshot(false).await;
    assert_eq!(collects.load(Ordering::SeqCst), 3);
    p.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_single_flight() {
    let n = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(std::sync::Barrier::new(2));
    let (count, gate) = (n.clone(), release.clone());
    let f = Fetcher {
        want_claude: true,
        now: hook(|| 1_000),
        claude_token: hook(move || {
            count.fetch_add(1, Ordering::SeqCst);
            gate.wait();
            Err("no token".into())
        }),
        ..Fetcher::default()
    };
    let p = Poller::new(Arc::new(f), None);
    let first = tokio::spawn({
        let p = p.clone();
        async move { p.refresh().await }
    });
    while n.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let second = tokio::spawn({
        let p = p.clone();
        async move { p.refresh().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.wait();
    let (a, b) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!(n.load(Ordering::SeqCst), 1, "collects");
    assert_eq!(a, b);
    p.stop();
}

#[tokio::test]
async fn refresh_cleans_up_after_panic() {
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let f = Fetcher {
        want_claude: true,
        now: hook(|| 1),
        claude_token: hook(move || {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("collect boom");
            }
            Err("no token".into())
        }),
        ..Fetcher::default()
    };
    let p = Poller::new(Arc::new(f), None);
    let crashed = tokio::spawn({
        let p = p.clone();
        async move { p.refresh().await }
    })
    .await;
    assert!(crashed.is_err());
    tokio::time::timeout(Duration::from_secs(2), p.refresh())
        .await
        .expect("Refresh blocked after a panicking collect");
    p.stop();
}

#[tokio::test]
async fn mark_cursor_connecting_clears_hint() {
    let p = Poller::new(Arc::new(Fetcher::default()), None);
    p.mark_cursor_connecting(true);
    let q = p.last().providers.unwrap()[0].clone();
    assert!(!q.hint.is_empty() && q.connecting);
    p.mark_cursor_connecting(false);
    let q = p.last().providers.unwrap()[0].clone();
    assert!(!q.connecting && q.hint.is_empty());
}

#[tokio::test]
async fn forget_provider_drops_row() {
    let p = Poller::new(Arc::new(Fetcher::default()), None);
    let row = |id: &str, w: &str| ProviderQuota {
        id: id.into(),
        windows: vec![Window {
            id: w.into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    p.set_last(Snapshot {
        providers: Some(vec![
            row(PROVIDER_CLAUDE, WINDOW_FIVE_HOUR),
            row(PROVIDER_CURSOR, "billing"),
        ]),
        ..Default::default()
    });
    p.forget_provider(PROVIDER_CURSOR);
    let ids: Vec<String> = p
        .last()
        .providers
        .unwrap()
        .into_iter()
        .map(|q| q.id)
        .collect();
    assert_eq!(ids, [PROVIDER_CLAUDE]);
}

#[tokio::test]
async fn mark_cursor_connect_failed() {
    let p = Poller::new(Arc::new(Fetcher::default()), None);
    p.mark_cursor_connecting(true);
    p.mark_cursor_connect_failed("Sign-in timed out: try Connect again");
    let q = p.last().providers.unwrap()[0].clone();
    assert!(!q.connecting && !q.error.is_empty() && !q.hint.is_empty());
}

#[tokio::test]
async fn disconnect_revokes_and_removes_tokens() {
    let (base, seen) = serve(|r| match r.path.as_str() {
        "/revoke" => ok("{}"),
        _ => (404, String::new()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    write_cursor_auth_file(
        &cursor_auth_path(dir.path()),
        &CursorTok {
            access: "sess".into(),
            api_key: "crsr_k".into(),
            api_key_id: 7,
            ..Default::default()
        },
    )
    .unwrap();
    let f = Fetcher {
        cursor_revoke_key_url: format!("{base}/revoke"),
        data_dir: hook(move || Some(path.clone())),
        want: hook(|| (false, false, false)),
        ..Fetcher::default()
    };
    let p = Poller::new(Arc::new(f), None);
    p.set_last(Snapshot {
        providers: Some(vec![ProviderQuota {
            id: PROVIDER_CURSOR.into(),
            ..Default::default()
        }]),
        ..Default::default()
    });
    p.disconnect_cursor().await.unwrap();
    assert!(!has_recur_cursor_auth_file(dir.path()));
    assert_eq!(seen.lock()[0].body, r#"{"id":7}"#);
    assert!(
        p.last()
            .providers
            .unwrap_or_default()
            .iter()
            .all(|q| q.id != PROVIDER_CURSOR)
    );
    p.stop();
}

// ---- provider usage

fn service(
    settings: Arc<Mutex<recur_domain::Settings>>,
    dir: Option<std::path::PathBuf>,
) -> Arc<UsageService> {
    let (read, write) = (settings.clone(), settings);
    UsageService::new(
        Arc::new(move || read.lock().clone()),
        Arc::new(move |s| {
            *write.lock() = s;
            Box::pin(async { Ok(()) })
        }),
        Arc::new(move || dir.clone()),
        None,
    )
}

#[tokio::test]
async fn get_provider_usage_without_poller() {
    let s = service(Arc::default(), None);
    let got = s.get_provider_usage().await;
    assert_eq!(got.providers, Some(vec![]));
    assert!(!s.connect_cursor_usage().error.is_empty());
    s.cancel_cursor_usage_connect();
    s.disconnect_cursor_usage().await.unwrap();
}

#[tokio::test]
async fn disconnect_cursor_usage_clears_settings_key() {
    let settings = Arc::new(Mutex::new(recur_domain::Settings {
        cursor_api_key: "crsr_leftover".into(),
        ..Default::default()
    }));
    let dir = tempfile::tempdir().unwrap();
    let s = service(settings.clone(), Some(dir.path().to_path_buf()));
    s.start();
    s.disconnect_cursor_usage().await.unwrap();
    assert_eq!(settings.lock().cursor_api_key, "");
    s.stop();
}

#[tokio::test]
async fn drop_orphan_cursor_key() {
    let settings = Arc::new(Mutex::new(recur_domain::Settings {
        cursor_api_key: "crsr_leftover".into(),
        ..Default::default()
    }));
    let s = service(settings.clone(), None);
    let empty = tempfile::tempdir().unwrap();
    let current = settings.lock().clone();
    let got = s.drop_orphan_cursor_key(current, empty.path()).await;
    assert_eq!(got.cursor_api_key, "");
    assert_eq!(settings.lock().cursor_api_key, "");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cursor-usage-auth.json"),
        r#"{"accessToken":"sess"}"#,
    )
    .unwrap();
    settings.lock().cursor_api_key = "crsr_kept".into();
    let current = settings.lock().clone();
    let got = s.drop_orphan_cursor_key(current, dir.path()).await;
    assert_eq!(got.cursor_api_key, "crsr_kept");
}

#[test]
fn service_wants_and_stub() {
    let settings = Arc::new(Mutex::new(recur_domain::Settings::default()));
    let dir = tempfile::tempdir().unwrap();
    let s = service(settings.clone(), Some(dir.path().to_path_buf()));
    assert_eq!(s.wants(), (true, false, false));
    assert_eq!(s.cursor_stub(), Some(cursor_no_session_quota()));
    settings.lock().codex_chat_gpt_auth = true;
    write_cursor_auth_file(
        &cursor_auth_path(dir.path()),
        &CursorTok {
            access: "a".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(s.wants(), (true, true, true));
    assert_eq!(s.cursor_stub(), Some(cursor_connect_tokens_quota()));
    settings.lock().disabled_providers = vec!["cursor".into(), "claude-code".into()];
    assert_eq!(s.wants(), (false, true, false));
}

#[tokio::test]
async fn probes_report_their_provider() {
    let f = Arc::new(Fetcher {
        claude_token: hook(|| Err("x".into())),
        cursor_token: hook(|| Err("x".into())),
        codex_auth: hook(|| Err("x".into())),
        ..Fetcher::default()
    });
    let probes: Vec<Box<dyn recur_agent::UsageProbe>> = vec![
        Box::new(ClaudeProbe(f.clone())),
        Box::new(CodexProbe(f.clone())),
        Box::new(CursorProbe(f)),
    ];
    for p in probes {
        let q = p.fetch().await;
        assert_eq!(q.id, p.provider_id());
        assert!(!q.error.is_empty());
    }
}

mod golden;
