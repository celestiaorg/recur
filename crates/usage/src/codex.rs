//! Codex / ChatGPT rate limits and monthly spend controls (Go
//! `usage/codex.go`).

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;

use crate::ctx::Ctx;
use crate::fetcher::Fetcher;
use crate::http::{do_json, http_err, is_auth_err, is_rate_limited};
use crate::parse::{
    Obj, as_bool, as_float, as_string, decode_map, email_from_jwt, first_any, first_map,
    first_string, jwt_payload, map_of, parse_resets_at, used_pct_from,
};
use crate::types::{
    PROVIDER_CODEX, ProviderQuota, SOURCE_CODEX_API_KEY, SOURCE_CODEX_CHATGPT, WINDOW_BILLING,
    WINDOW_EXTRA, WINDOW_FIVE_HOUR, WINDOW_SEVEN_DAY, Window, clamp_pct, set_source,
};

pub const DEFAULT_CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// The Codex `auth.json` login.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexAuth {
    pub access_token: String,
    pub id_token: String,
    pub account_id: String,
    pub email: String,
    pub has_api_key: bool,
    pub has_chatgpt: bool,
}

fn quota() -> ProviderQuota {
    ProviderQuota {
        id: PROVIDER_CODEX.into(),
        name: "Codex".into(),
        ..Default::default()
    }
}

/// Recur's own `CODEX_HOME` first, then `~/.codex`. A file without a ChatGPT
/// access token falls through to the next.
pub(crate) fn default_codex_auth(data_dir: Option<&Path>) -> Result<CodexAuth, String> {
    let mut last_err = None;
    let mut try_file = |p: &Path| match read_codex_auth_file(p) {
        Ok(a) if !a.access_token.is_empty() => Some(a),
        Ok(_) => None,
        Err(e) => {
            last_err = Some(e);
            None
        }
    };
    if let Some(dir) = data_dir
        && let Some(a) = try_file(&dir.join("codex-home").join("auth.json"))
    {
        return Ok(a);
    }
    if let Ok(home) = recur_project::home::user_home_dir()
        && let Some(a) = try_file(&Path::new(&home).join(".codex").join("auth.json"))
    {
        return Ok(a);
    }
    Err(last_err.unwrap_or_else(|| "no Codex ChatGPT auth.json".into()))
}

fn read_codex_auth_file(path: &Path) -> Result<CodexAuth, String> {
    let raw = std::fs::read(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    parse_codex_auth(&raw)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthFile {
    tokens: Option<AuthTokens>,
    id_token: Option<String>,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthTokens {
    access_token: Option<String>,
    #[allow(dead_code)]
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

pub(crate) fn parse_codex_auth(raw: &[u8]) -> Result<CodexAuth, String> {
    let file: AuthFile = match serde_json::from_slice::<Option<AuthFile>>(raw) {
        Ok(f) => f.unwrap_or_default(),
        Err(e) => return Err(format!("codex auth.json: {e}")),
    };
    let s = |v: &Option<String>| v.as_deref().unwrap_or_default().trim().to_owned();
    let mut out = CodexAuth {
        email: s(&file.email),
        ..Default::default()
    };
    if let Some(t) = &file.tokens {
        out.access_token = s(&t.access_token);
        out.id_token = s(&t.id_token);
        out.account_id = s(&t.account_id);
        out.has_chatgpt = !out.access_token.is_empty();
        if out.email.is_empty() {
            out.email = email_from_jwt(t.id_token.as_deref().unwrap_or_default());
        }
        if out.email.is_empty() {
            out.email = email_from_jwt(t.access_token.as_deref().unwrap_or_default());
        }
    }
    if out.id_token.is_empty() {
        out.id_token = s(&file.id_token);
    }
    if out.email.is_empty() {
        out.email = email_from_jwt(file.id_token.as_deref().unwrap_or_default());
    }
    if out.account_id.is_empty() {
        out.account_id = account_id_from_jwt(&out.access_token);
    }
    if out.account_id.is_empty() {
        out.account_id = account_id_from_jwt(&out.id_token);
    }
    if !s(&file.openai_api_key).is_empty() {
        out.has_api_key = true;
    }
    if !out.has_chatgpt && !out.has_api_key {
        return Err("codex auth.json has no tokens".into());
    }
    Ok(out)
}

pub(crate) async fn fetch_codex(ctx: &Ctx, f: &Fetcher) -> ProviderQuota {
    let mut out = quota();
    let auth = match &f.codex_auth {
        Some(h) => h(),
        None => default_codex_auth(f.data_dir().as_deref()),
    };
    let auth = match auth {
        Ok(a) if a.has_chatgpt || a.has_api_key => a,
        _ => {
            out.error = "Not signed in to Codex".into();
            out.hint = "Sign in with ChatGPT in Settings → Providers → Codex.".into();
            return out;
        }
    };
    if auth.has_api_key && !auth.has_chatgpt {
        set_source(&mut out, SOURCE_CODEX_API_KEY);
        out.error = "API key billing — no plan quota".into();
        out.hint = "Codex API keys are pay-as-you-go. Sign in with ChatGPT to see 5-hour and 7-day limits.".into();
        return out;
    }
    set_source(&mut out, SOURCE_CODEX_CHATGPT);
    let mut account = auth.account_id.clone();
    let headers = |account: &str| {
        vec![
            ("Authorization", format!("Bearer {}", auth.access_token)),
            ("Accept", "application/json".to_owned()),
            ("originator", "codex_cli_rs".to_owned()),
            ("User-Agent", "codex_cli_rs".to_owned()),
            ("ChatGPT-Account-Id", account.to_owned()),
        ]
    };
    let url = f.url(&f.codex_usage_url, DEFAULT_CODEX_USAGE_URL);
    let (status, raw) =
        match do_json(ctx, &f.http, Method::GET, url, None, &headers(&account)).await {
            Ok(r) => r,
            Err(e) => {
                out.error = "Could not reach ChatGPT".into();
                out.hint = e;
                return out;
            }
        };
    if is_rate_limited(status) {
        let mut q = quota();
        q.error = "rate_limited".into();
        return q;
    }
    if is_auth_err(status) {
        out.error = "Codex login expired".into();
        out.hint = "Sign in with ChatGPT again in Settings → Providers → Codex.".into();
        return out;
    }
    if status >= 300 {
        out.error = "Codex usage unavailable".into();
        out.hint = http_err(status, &raw);
        return out;
    }
    let Ok(mut q) = parse_codex_usage(&raw) else {
        out.error = "Codex usage response was unreadable".into();
        return out;
    };
    let resolved = resolve_codex_account_id(&auth, &raw);
    if !resolved.is_empty() && account.is_empty() {
        account.clone_from(&resolved);
    }
    if !q.windows.iter().any(|w| w.id == WINDOW_BILLING)
        && codex_spend_control_present(&raw)
        && let Some(monthly) = fetch_codex_monthly(ctx, f, &headers(&account), &resolved).await
    {
        q.windows.push(monthly);
    }
    set_source(&mut q, SOURCE_CODEX_CHATGPT);
    if q.email.is_empty() {
        q.email = auth.email;
    }
    q
}

async fn fetch_codex_monthly(
    ctx: &Ctx,
    f: &Fetcher,
    headers: &[(&str, String)],
    account_id: &str,
) -> Option<Window> {
    let url = if f.codex_monthly_url.is_empty() {
        codex_monthly_usage_url(
            f.url(&f.codex_usage_url, DEFAULT_CODEX_USAGE_URL),
            account_id,
        )
    } else {
        f.codex_monthly_url.clone()
    };
    if url.is_empty() {
        return None;
    }
    let (status, raw) = do_json(ctx, &f.http, Method::GET, &url, None, headers)
        .await
        .ok()?;
    if status >= 300 {
        return None;
    }
    parse_codex_monthly_usage(&raw)
}

pub(crate) fn parse_codex_usage(raw: &[u8]) -> Result<ProviderQuota, String> {
    let m = decode_map(raw).map_err(|e| format!("codex usage: {e}"))?;
    let mut q = quota();
    set_source(&mut q, SOURCE_CODEX_CHATGPT);
    let plan = first_string(&m, &["plan_type", "planType"]);
    if !plan.is_empty() {
        q.plan = format_codex_plan(&plan);
    }
    q.email = first_string(&m, &["email", "user_email", "userEmail"]);
    let rl = map_of(m.get("rate_limit")).unwrap_or(&m);
    if let Some(w) =
        window_from_codex_rate_limit(map_of(rl.get("primary_window")), WINDOW_FIVE_HOUR, "5-hour")
    {
        q.windows.push(w);
    }
    if let Some(w) = window_from_codex_rate_limit(
        map_of(rl.get("secondary_window")),
        WINDOW_SEVEN_DAY,
        "7-day",
    ) {
        q.windows.push(w);
    }
    if let Some(monthly) = parse_codex_individual_limit(&m) {
        q.windows.push(monthly);
    }
    if let Some(credits) = map_of(m.get("credits"))
        && as_bool(credits.get("has_credits"))
        && !as_bool(credits.get("unlimited"))
    {
        let detail = as_string(credits.get("balance"));
        if !detail.is_empty() {
            q.windows.push(Window {
                id: WINDOW_EXTRA.into(),
                label: "Credits".into(),
                used_pct: 0.0,
                resets_at: 0,
                detail,
            });
        }
    }
    if q.windows.is_empty() {
        return Err("codex usage: no windows".into());
    }
    Ok(q)
}

fn codex_spend_control_present(raw: &[u8]) -> bool {
    decode_map(raw).is_ok_and(|m| first_map(&m, &["spend_control", "spendControl"]).is_some())
}

pub(crate) fn resolve_codex_account_id(auth: &CodexAuth, usage: &[u8]) -> String {
    let id = auth.account_id.trim();
    if !id.is_empty() {
        return id.to_owned();
    }
    if let Ok(m) = decode_map(usage) {
        let id = first_string(&m, &["account_id", "accountId"]);
        if !id.is_empty() {
            return id;
        }
    }
    let id = account_id_from_jwt(&auth.access_token);
    if !id.is_empty() {
        return id;
    }
    account_id_from_jwt(&auth.id_token)
}

fn account_id_from_jwt(tok: &str) -> String {
    let Some(m) = jwt_payload(tok) else {
        return String::new();
    };
    if let Some(auth) = map_of(m.get("https://api.openai.com/auth")) {
        let id = first_string(auth, &["chatgpt_account_id", "account_id", "accountId"]);
        if !id.is_empty() {
            return id;
        }
    }
    first_string(&m, &["chatgpt_account_id", "account_id", "accountId"])
}

fn parse_codex_individual_limit(m: &Obj) -> Option<Window> {
    let limit = first_map(m, &["individual_limit", "individualLimit"])
        .or_else(|| {
            first_map(m, &["spend_control", "spendControl"])
                .and_then(|sc| first_map(sc, &["individual_limit", "individualLimit"]))
        })
        .or_else(|| {
            map_of(m.get("rate_limit"))
                .and_then(|rl| first_map(rl, &["individual_limit", "individualLimit"]))
        });
    window_from_codex_limit(limit)
}

pub(crate) fn parse_codex_monthly_usage(raw: &[u8]) -> Option<Window> {
    let m = decode_map(raw).ok()?;
    let eff = first_map(&m, &["effective_monthly_limit", "effectiveMonthlyLimit"])?;
    if !codex_limit_enforced(&first_string(eff, &["enforcement_mode", "enforcementMode"])) {
        return None;
    }
    let used = as_float(first_any(&m, &["current_month_usage", "currentMonthUsage"]));
    let cap = as_float(first_any(eff, &["limit"]));
    window_from_codex_credits(used, cap, 0)
}

fn window_from_codex_rate_limit(
    block: Option<&Obj>,
    fallback_id: &str,
    fallback_label: &str,
) -> Option<Window> {
    let block = block?;
    let pct = used_pct_from(Some(block))?;
    let (id, label) = match as_float(first_any(
        block,
        &["limit_window_seconds", "limitWindowSeconds"],
    )) as i64
    {
        18_000 => (WINDOW_FIVE_HOUR, "5-hour"),
        604_800 => (WINDOW_SEVEN_DAY, "7-day"),
        _ => (fallback_id, fallback_label),
    };
    Some(Window {
        id: id.into(),
        label: label.into(),
        used_pct: pct,
        resets_at: parse_resets_at(first_any(block, &["reset_at", "resets_at", "resetAt"])),
        detail: String::new(),
    })
}

fn window_from_codex_limit(limit: Option<&Obj>) -> Option<Window> {
    let limit = limit?;
    if !codex_limit_enforced(&first_string(
        limit,
        &["enforcement_mode", "enforcementMode"],
    )) {
        return None;
    }
    let cap = as_float(first_any(limit, &["limit"]));
    let used = as_float(first_any(limit, &["used"]));
    let resets = parse_resets_at(first_any(
        limit,
        &["reset_at", "resets_at", "resetAt", "resetsAt"],
    ));
    let pct = match used_pct_from(Some(limit)) {
        Some(p) => p,
        None if cap > 0.0 => clamp_pct(used / cap * 100.0),
        None => return None,
    };
    Some(Window {
        id: WINDOW_BILLING.into(),
        label: "Monthly".into(),
        used_pct: pct,
        resets_at: resets,
        detail: if cap > 0.0 {
            format_codex_credits(used, cap)
        } else {
            String::new()
        },
    })
}

fn window_from_codex_credits(used: f64, cap: f64, resets: i64) -> Option<Window> {
    if cap <= 0.0 {
        return None;
    }
    Some(Window {
        id: WINDOW_BILLING.into(),
        label: "Monthly".into(),
        used_pct: clamp_pct(used / cap * 100.0),
        resets_at: resets,
        detail: format_codex_credits(used, cap),
    })
}

pub(crate) fn format_codex_credits(used: f64, cap: f64) -> String {
    format!(
        "{} of {} credits",
        format_codex_credit_qty(used),
        format_codex_credit_qty(cap)
    )
}

/// Rounded half away from zero, thousands separated by commas.
pub(crate) fn format_codex_credit_qty(n: f64) -> String {
    let n = n.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The spend-controls URL next to the usage URL:
/// `…/backend-api/accounts/<id>/spend-controls/current-user/monthly-usage`.
pub(crate) fn codex_monthly_usage_url(usage_url: &str, account_id: &str) -> String {
    let account_id = account_id.trim();
    if usage_url.is_empty() || account_id.is_empty() {
        return String::new();
    }
    let Ok(mut u) = url::Url::parse(usage_url) else {
        return String::new();
    };
    let path = u.path().trim_end_matches('/').to_owned();
    let path = path.strip_suffix("/wham/usage").unwrap_or(&path);
    let path = format!(
        "{}/accounts/{account_id}/spend-controls/current-user/monthly-usage",
        path.trim_end_matches('/')
    );
    u.set_path(&path);
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

fn codex_limit_enforced(mode: &str) -> bool {
    !matches!(
        mode.trim().to_lowercase().as_str(),
        "none" | "off" | "disabled" | "no_limit" | "nolimit" | "unlimited"
    )
}

/// Codex `planType` / ChatGPT `plan_type` keys to CLI display names (Go
/// `codex.FormatPlan`).
pub fn format_codex_plan(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "" => "",
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro Lite",
        "team" => "Team",
        "self_serve_business_prolite" => "Self Serve Business ProLite",
        "self_serve_business_usage_based" => "Self Serve Business Usage Based",
        "business" => "Business",
        "ent26" | "enterprise" | "hc" => "Enterprise",
        "enterprise_cbp_automation" => "Enterprise (Automation)",
        "enterprise_cbp_usage_based" => "Enterprise CBP Usage Based",
        "education" | "edu" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        _ => return raw.trim().to_owned(),
    }
    .to_owned()
}
