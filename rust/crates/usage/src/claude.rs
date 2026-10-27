//! Claude subscription quota from the OAuth usage endpoint, and the Claude
//! Code login on this machine (Go `usage/claude.go`, `claude_creds.go`).

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use reqwest::Method;

use crate::ctx::Ctx;
use crate::fetcher::Fetcher;
use crate::http::{do_json, http_err, is_auth_err, is_rate_limited};
use crate::parse::{
    Obj, as_bool, as_float, as_string, decode_map, first_any, first_map, first_string, map_of,
    parse_resets_at, slice_of, used_pct_from,
};
use crate::types::{
    PROVIDER_CLAUDE, ProviderQuota, SOURCE_CLAUDE_LOGIN, WINDOW_EXTRA, WINDOW_FABLE,
    WINDOW_FIVE_HOUR, WINDOW_OPUS, WINDOW_SEVEN_DAY, WINDOW_SONNET, Window, clamp_pct, set_source,
};

pub const DEFAULT_CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// Rotate before the usage API 401s. Access tokens last hours, not days; two
/// minutes is enough for one poller tick.
const CLAUDE_ACCESS_LEEWAY_MS: i64 = 2 * 60 * 1000;
const CLAUDE_NUDGE_TIMEOUT: Duration = Duration::from_secs(5);
/// Left on the Claude context so the usage request still runs after Keychain
/// and `claude auth status`; those steps used to take the full 5 s cap each
/// and spend the collect budget before the request.
const CLAUDE_HTTP_RESERVE: Duration = Duration::from_secs(4);

fn quota() -> ProviderQuota {
    ProviderQuota {
        id: PROVIDER_CLAUDE.into(),
        name: "Claude".into(),
        ..Default::default()
    }
}

/// The Claude Code login blob (Keychain on macOS, credential files
/// elsewhere).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeOAuth {
    pub access: String,
    pub refresh: String,
    /// Unix ms; 0 when unknown.
    pub expires_at: i64,
}

impl ClaudeOAuth {
    fn has_login(&self) -> bool {
        !self.access.is_empty() || !self.refresh.is_empty()
    }
}

pub(crate) async fn fetch_claude(ctx: &Ctx, f: &Fetcher) -> ProviderQuota {
    let mut out = quota();
    let (tok, nudged) = match claude_access(ctx, f).await {
        Ok((tok, nudged)) if !tok.is_empty() => (tok, nudged),
        _ => {
            out.error = "Not signed in to Claude Code".into();
            out.hint = "Run claude login, then reopen Recur.".into();
            return out;
        }
    };
    let (q, status) = fetch_claude_usage(ctx, f, &tok).await;
    // 401 with a still-unexpired token: one rotation, then retry if the access
    // token actually changed. Skip a second `claude auth status` when the
    // proactive expiry nudge already ran, and skip 403 (org/policy).
    if status != 401 || nudged || f.claude_token.is_some() {
        return q;
    }
    nudge_claude(ctx, f).await;
    let next = claude_creds(ctx, f).await.access;
    if next.is_empty() || next == tok {
        return q;
    }
    fetch_claude_usage(ctx, f, &next).await.0
}

/// A usable access token, and whether this call already ran the nudge.
async fn claude_access(ctx: &Ctx, f: &Fetcher) -> Result<(String, bool), String> {
    if let Some(h) = &f.claude_token {
        return h().map(|t| (t, false));
    }
    let mut creds = claude_creds(ctx, f).await;
    let mut nudged = false;
    if claude_access_needs_refresh(&creds, f.now_ms()) {
        nudge_claude(ctx, f).await;
        nudged = true;
        let next = claude_creds(ctx, f).await;
        if !next.access.is_empty() {
            creds = next;
        }
    }
    if creds.access.is_empty() {
        return Err("no Claude Code OAuth token on this machine".into());
    }
    Ok((creds.access, nudged))
}

async fn claude_creds(ctx: &Ctx, f: &Fetcher) -> ClaudeOAuth {
    match &f.claude_creds {
        Some(h) => h(),
        None => default_claude_creds(ctx).await,
    }
}

async fn nudge_claude(ctx: &Ctx, f: &Fetcher) {
    match &f.nudge_claude {
        Some(h) => h(),
        None => default_nudge_claude_auth(ctx).await,
    }
}

async fn fetch_claude_usage(ctx: &Ctx, f: &Fetcher, tok: &str) -> (ProviderQuota, u16) {
    let mut out = quota();
    let url = f.url(&f.claude_usage_url, DEFAULT_CLAUDE_USAGE_URL);
    let res = do_json(
        ctx,
        &f.http,
        Method::GET,
        url,
        None,
        &[
            ("Authorization", format!("Bearer {tok}")),
            ("anthropic-beta", "oauth-2025-04-20".into()),
            ("User-Agent", "claude-code/2.1.80".into()),
            ("Accept", "application/json".into()),
        ],
    )
    .await;
    let (status, raw) = match res {
        Ok(r) => r,
        Err(e) => {
            out.error = "Could not reach Anthropic".into();
            out.hint = e;
            return (out, 0);
        }
    };
    if is_rate_limited(status) {
        out.error = "rate_limited".into();
        return (out, status);
    }
    if is_auth_err(status) {
        out.error = "Claude Code login expired".into();
        out.hint = "Run claude login, then reopen Recur.".into();
        return (out, status);
    }
    if status >= 300 {
        out.error = "Claude usage unavailable".into();
        out.hint = http_err(status, &raw);
        return (out, status);
    }
    let Ok(mut q) = parse_claude_usage(&raw) else {
        out.error = "Claude usage response was unreadable".into();
        return (out, status);
    };
    set_source(&mut q, SOURCE_CLAUDE_LOGIN);
    if q.email.is_empty() {
        q.email = default_claude_account();
    }
    (q, status)
}

/// The usage payload: top-level windows, `limits[]`, model-scoped rows and
/// extra (pay-as-you-go) credits.
pub(crate) fn parse_claude_usage(raw: &[u8]) -> Result<ProviderQuota, String> {
    let m = decode_map(raw).map_err(|e| format!("claude usage: {e}"))?;
    let mut q = quota();
    set_source(&mut q, SOURCE_CLAUDE_LOGIN);
    q.email = first_string(&m, &["email", "user_email"]);
    if q.email.is_empty()
        && let Some(org) = map_of(m.get("organization"))
    {
        q.email = as_string(org.get("email"));
    }
    let plan = as_string(m.get("subscription_type"));
    if !plan.is_empty() {
        q.plan = plan;
    }
    let specs: [(&[&str], &str, &str); 5] = [
        (
            &["five_hour", "fiveHour", "session"],
            WINDOW_FIVE_HOUR,
            "5-hour",
        ),
        (
            &["seven_day", "sevenDay", "weekly"],
            WINDOW_SEVEN_DAY,
            "7-day",
        ),
        (
            &["seven_day_opus", "sevenDayOpus"],
            WINDOW_OPUS,
            "Opus (7-day)",
        ),
        (
            &["seven_day_sonnet", "sevenDaySonnet"],
            WINDOW_SONNET,
            "Sonnet (7-day)",
        ),
        (
            &["seven_day_fable", "sevenDayFable"],
            WINDOW_FABLE,
            "Fable (7-day)",
        ),
    ];
    for (keys, id, label) in specs {
        let Some(block) = first_map(&m, keys) else {
            continue;
        };
        let Some(pct) = used_pct_from(Some(block)) else {
            continue;
        };
        q.windows.push(Window {
            id: id.into(),
            label: label.into(),
            used_pct: pct,
            resets_at: parse_resets_at(first_any(block, &["resets_at", "resetsAt", "reset_at"])),
            detail: String::new(),
        });
    }
    let mut inactive = HashSet::new();
    append_claude_limits(&mut q, &m, &mut inactive);
    append_claude_model_scoped(&mut q, &m, &mut inactive);
    if let Some(extra) = parse_claude_extra(first_map(&m, &["extra", "extra_usage", "extraUsage"]))
    {
        q.windows.push(extra);
    }
    if q.windows.is_empty() {
        return Err("claude usage: no windows".into());
    }
    Ok(q)
}

fn parse_claude_extra(block: Option<&Obj>) -> Option<Window> {
    let block = block?;
    if let Some(v) = first_any(block, &["is_enabled", "isEnabled"])
        && !as_bool(Some(v))
    {
        return None;
    }
    let mut w = Window {
        id: WINDOW_EXTRA.into(),
        label: "Extra".into(),
        used_pct: used_pct_from(Some(block)).unwrap_or(0.0),
        resets_at: parse_resets_at(first_any(block, &["resets_at", "resetsAt", "reset_at"])),
        detail: String::new(),
    };
    let used = as_float(first_any(block, &["used_credits", "usedCredits"]));
    let limit = as_float(first_any(block, &["monthly_limit", "monthlyLimit"]));
    if limit > 0.0 {
        w.used_pct = clamp_pct(used / limit * 100.0);
        w.detail = format!("${:.2} of ${:.2}", used / 100.0, limit / 100.0);
    }
    if w.used_pct == 0.0 && w.detail.is_empty() && w.resets_at == 0 {
        return None;
    }
    Some(w)
}

fn append_claude_limits(q: &mut ProviderQuota, m: &Obj, inactive: &mut HashSet<String>) {
    for row in slice_of(m.get("limits")) {
        let Some(rm) = row.as_object() else { continue };
        let kind = first_string(rm, &["kind"]);
        let Some(pct) = used_pct_from(Some(rm)) else {
            continue;
        };
        let active = claude_limit_active(rm);
        let resets = parse_resets_at(first_any(rm, &["resets_at", "resetsAt", "reset_at"]));
        match kind.as_str() {
            // is_active marks the binding cap. A false session/weekly_all row is
            // still the real 5-hour/7-day bucket when a scoped cap is binding.
            "session" => {
                append_claude_window(q, WINDOW_FIVE_HOUR, "5-hour", pct, resets, active, inactive)
            }
            "weekly_all" | "weekly" => {
                append_claude_window(q, WINDOW_SEVEN_DAY, "7-day", pct, resets, active, inactive)
            }
            "weekly_scoped" => append_scoped_claude_window(
                q,
                &claude_scope_name(rm),
                pct,
                resets,
                active,
                inactive,
            ),
            _ => {}
        }
    }
}

fn append_claude_model_scoped(q: &mut ProviderQuota, m: &Obj, inactive: &mut HashSet<String>) {
    let mut rows = slice_of(first_any(m, &["model_scoped", "modelScoped"]));
    if rows.is_empty()
        && let Some(rl) = map_of(first_any(m, &["rate_limits", "rateLimits"]))
    {
        rows = slice_of(first_any(rl, &["model_scoped", "modelScoped"]));
    }
    for row in rows {
        let Some(rm) = row.as_object() else { continue };
        if !claude_model_scoped_weekly(rm) {
            continue;
        }
        let Some(pct) = used_pct_from(Some(rm)) else {
            continue;
        };
        let resets = parse_resets_at(first_any(rm, &["resets_at", "resetsAt", "reset_at"]));
        append_scoped_claude_window(
            q,
            &claude_scope_name(rm),
            pct,
            resets,
            claude_limit_active(rm),
            inactive,
        );
    }
}

fn append_scoped_claude_window(
    q: &mut ProviderQuota,
    name: &str,
    pct: f64,
    resets: i64,
    active: bool,
    inactive: &mut HashSet<String>,
) {
    if name.is_empty() {
        return;
    }
    let id = scoped_window_id(name);
    append_claude_window(
        q,
        &id,
        &scoped_window_label(name),
        pct,
        resets,
        active,
        inactive,
    );
}

/// Adds a window once per id. A later active row replaces an earlier
/// inactive one with the same id.
fn append_claude_window(
    q: &mut ProviderQuota,
    id: &str,
    label: &str,
    pct: f64,
    resets: i64,
    active: bool,
    inactive: &mut HashSet<String>,
) {
    if id.is_empty() {
        return;
    }
    let w = Window {
        id: id.into(),
        label: label.into(),
        used_pct: pct,
        resets_at: resets,
        detail: String::new(),
    };
    if let Some(i) = q.windows.iter().position(|w| w.id == id) {
        if active && inactive.remove(id) {
            q.windows[i] = w;
        }
        return;
    }
    q.windows.push(w);
    if !active {
        inactive.insert(id.to_owned());
    }
}

fn claude_limit_active(rm: &Obj) -> bool {
    match first_any(rm, &["is_active", "isActive"]) {
        None => true,
        v => as_bool(v),
    }
}

fn claude_model_scoped_weekly(rm: &Obj) -> bool {
    let kind = first_string(rm, &["kind", "period", "window"])
        .trim()
        .to_lowercase();
    match kind.as_str() {
        "" => true,
        "session" | "five_hour" | "5h" | "5-hour" => false,
        "weekly_scoped" | "weekly" | "weekly_all" | "seven_day" | "7d" | "7-day" => true,
        other => other.contains("week") || other.contains("seven"),
    }
}

fn claude_scope_name(rm: &Obj) -> String {
    if let Some(scope) = map_of(rm.get("scope"))
        && let Some(model) = map_of(scope.get("model"))
    {
        let name = first_string(model, &["display_name", "displayName", "name"]);
        if !name.is_empty() {
            return name;
        }
    }
    first_string(rm, &["display_name", "displayName", "name"])
}

pub(crate) fn scoped_window_id(name: &str) -> String {
    let s = canonical_scoped_name(name);
    match s.as_str() {
        "" => String::new(),
        "fable" => WINDOW_FABLE.into(),
        "opus" => WINDOW_OPUS.into(),
        "sonnet" => WINDOW_SONNET.into(),
        _ => format!("seven_day_{s}"),
    }
}

fn canonical_scoped_name(name: &str) -> String {
    let s = name.trim().to_lowercase().replace([' ', '-'], "_");
    s.strip_prefix("claude_").map(str::to_owned).unwrap_or(s)
}

pub(crate) fn scoped_window_label(name: &str) -> String {
    let s = canonical_scoped_name(name);
    if s.is_empty() {
        return format!("{} (7-day)", name.trim());
    }
    let Some((fam, rest)) = split_scoped_family(&s) else {
        return format!("{} (7-day)", humanize_scoped_name(&s));
    };
    let mut label = fam.to_owned();
    if !rest.is_empty() {
        label.push(' ');
        label.push_str(&pretty_scoped_version(rest));
    }
    format!("{label} (7-day)")
}

fn split_scoped_family(s: &str) -> Option<(&'static str, &str)> {
    for name in ["Fable", "Opus", "Sonnet"] {
        let key = name.to_lowercase();
        if s == key {
            return Some((name, ""));
        }
        if let Some(rest) = s.strip_prefix(&format!("{key}_"))
            && !rest.is_empty()
        {
            return Some((name, rest));
        }
    }
    None
}

fn pretty_scoped_version(rest: &str) -> String {
    if is_model_id_suffix(rest) {
        rest.replace('_', ".")
    } else {
        rest.replace('_', " ")
    }
}

fn humanize_scoped_name(s: &str) -> String {
    s.split('_')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(first) if first.is_ascii() => {
                    first.to_ascii_uppercase().to_string() + c.as_str()
                }
                // Go uppercases the first byte only; a non-ASCII first byte is
                // left as is.
                Some(first) => first.to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_model_id_suffix(s: &str) -> bool {
    let mut has_digit = false;
    for c in s.chars() {
        if c == '_' {
            continue;
        }
        if !c.is_ascii_digit() {
            return false;
        }
        has_digit = true;
    }
    has_digit
}

// ---- credentials

async fn default_claude_creds(ctx: &Ctx) -> ClaudeOAuth {
    let keychain = if cfg!(target_os = "macos") {
        read_claude_keychain_creds(ctx).await.unwrap_or_default()
    } else {
        ClaudeOAuth::default()
    };
    let mut file = ClaudeOAuth::default();
    for path in claude_credential_paths() {
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let c = parse_claude_oauth(&raw);
        if c.has_login() {
            file = c;
            break;
        }
    }
    pick_claude_creds(keychain, file)
}

/// The Keychain item's file counterparts. `CLAUDE_CONFIG_DIR` (when set) is
/// searched first so a leftover `$HOME/.claude` blob cannot shadow the live
/// login.
pub(crate) fn claude_credential_paths() -> Vec<String> {
    claude_credential_paths_in(
        &std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default(),
        recur_project::home::user_home_dir().ok().as_deref(),
    )
}

pub(crate) fn claude_credential_paths_in(config_dir: &str, home: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: String| {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    let dir = config_dir.trim();
    if !dir.is_empty() {
        add(join(dir, ".credentials.json"));
        add(join(dir, "credentials.json"));
    }
    if let Some(home) = home {
        add(join(home, ".claude/.credentials.json"));
        add(join(home, ".claude/credentials.json"));
        add(join(home, ".claude.json"));
    }
    out
}

fn join(dir: &str, rel: &str) -> String {
    Path::new(dir).join(rel).to_string_lossy().into_owned()
}

/// Prefers the blob with the later expiry so a leftover Keychain item cannot
/// shadow a newer file login. With unknown expiry the Keychain (the live
/// macOS store) wins.
pub(crate) fn pick_claude_creds(keychain: ClaudeOAuth, file: ClaudeOAuth) -> ClaudeOAuth {
    match (keychain.has_login(), file.has_login()) {
        (false, false) => ClaudeOAuth::default(),
        (false, true) => file,
        (true, false) => keychain,
        _ if keychain.expires_at > 0
            && file.expires_at > 0
            && file.expires_at > keychain.expires_at =>
        {
            file
        }
        _ if file.expires_at > 0 && keychain.expires_at == 0 => file,
        _ => keychain,
    }
}

async fn read_claude_keychain_creds(ctx: &Ctx) -> Result<ClaudeOAuth, String> {
    let step = claude_step_ctx(ctx, CLAUDE_NUDGE_TIMEOUT, CLAUDE_HTTP_RESERVE);
    let mut cmd = tokio::process::Command::new("security");
    cmd.args(["find-generic-password", "-s", CLAUDE_KEYCHAIN_SERVICE, "-w"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = step
        .run(cmd.output())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("security: {}", out.status));
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let c = parse_claude_oauth(raw.as_bytes());
    if c.has_login() {
        return Ok(c);
    }
    if looks_like_claude_token(&raw) {
        return Ok(ClaudeOAuth {
            access: raw,
            ..Default::default()
        });
    }
    Err("keychain item had no access token".into())
}

/// Asks the Claude Code CLI to rotate an expired access token. Recur does not
/// spend the single-use refresh token itself: that races Claude Code and can
/// wipe the Keychain login.
async fn default_nudge_claude_auth(ctx: &Ctx) {
    let step = claude_step_ctx(ctx, CLAUDE_NUDGE_TIMEOUT, CLAUDE_HTTP_RESERVE);
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let _ = step.run(cmd.status()).await;
}

/// Caps a CLI/Keychain step at `cap` without spending the `reserve` tail of
/// `ctx`, so the usage request is still reachable.
pub(crate) fn claude_step_ctx(ctx: &Ctx, cap: Duration, reserve: Duration) -> Ctx {
    let mut d = cap;
    if let Some(left) = ctx.remaining() {
        d = d.min(left.saturating_sub(reserve));
    }
    ctx.with_timeout(d)
}

pub(crate) fn claude_access_needs_refresh(c: &ClaudeOAuth, now_ms: i64) -> bool {
    if c.access.is_empty() {
        return !c.refresh.is_empty();
    }
    if c.expires_at <= 0 {
        return false;
    }
    now_ms + CLAUDE_ACCESS_LEEWAY_MS >= c.expires_at
}

pub(crate) fn parse_claude_oauth(raw: &[u8]) -> ClaudeOAuth {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim();
    if looks_like_claude_token(s) {
        return ClaudeOAuth {
            access: s.to_owned(),
            ..Default::default()
        };
    }
    let Ok(m) = decode_map(s.as_bytes()) else {
        return ClaudeOAuth::default();
    };
    let c = claude_oauth_from_map(&m);
    if c.has_login() {
        return c;
    }
    for k in [
        "claudeAiOauth",
        "claudeAiOAuth",
        "oauth",
        "oauthAccount",
        "claude",
    ] {
        if let Some(nested) = map_of(m.get(k)) {
            let c = claude_oauth_from_map(nested);
            if c.has_login() {
                return c;
            }
        }
    }
    ClaudeOAuth::default()
}

fn claude_oauth_from_map(m: &Obj) -> ClaudeOAuth {
    let c = ClaudeOAuth {
        access: first_string(m, &["accessToken", "access_token", "token"]),
        refresh: first_string(m, &["refreshToken", "refresh_token"]),
        expires_at: parse_resets_at(first_any(m, &["expiresAt", "expires_at"])),
    };
    if c.has_login() {
        return c;
    }
    for k in ["claudeAiOauth", "claudeAiOAuth"] {
        if let Some(nested) = map_of(m.get(k)) {
            return claude_oauth_from_map(nested);
        }
    }
    c
}

fn looks_like_claude_token(s: &str) -> bool {
    s.trim().starts_with("sk-ant-")
}

/// The local login identity Settings shows (`~/.claude.json`), not the usage
/// API's.
fn default_claude_account() -> String {
    let Ok(home) = recur_project::home::user_home_dir() else {
        return String::new();
    };
    for rel in [".claude.json", ".claude/config.json"] {
        if let Ok(raw) = std::fs::read(join(&home, rel)) {
            let email = parse_claude_account_file(&raw);
            if !email.is_empty() {
                return email;
            }
        }
    }
    String::new()
}

pub(crate) fn parse_claude_account_file(raw: &[u8]) -> String {
    let Ok(m) = decode_map(raw) else {
        return String::new();
    };
    let acc = map_of(m.get("oauthAccount"))
        .or_else(|| map_of(m.get("oauth_account")))
        .unwrap_or(&m);
    first_string(
        acc,
        &["emailAddress", "email_address", "email", "userEmail"],
    )
}
