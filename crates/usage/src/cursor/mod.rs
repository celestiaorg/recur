//! Cursor included-usage from the dashboard API with a Recur Connect session.

pub(crate) mod oauth;
pub(crate) mod session;

use reqwest::Method;

pub use session::{
    CursorTok, delete_recur_cursor_auth, has_recur_cursor_auth_file, recur_cursor_api_key,
};

use crate::connect::{cursor_connecting_quota, cursor_hint_no_session};
use crate::ctx::Ctx;
use crate::fetcher::Fetcher;
use crate::http::{do_json, http_err, is_auth_err, is_rate_limited};
use crate::parse::{
    Obj, as_float, as_int64, clamp_pct_field, decode_map, email_from_cursor_json, email_from_jwt,
    first_any, first_string, jwt_claim, jwt_needs_refresh, map_of, parse_resets_at, slice_of,
};
use crate::types::{
    BUCKET_CURSOR, BUCKET_OTHER, ModelRow, PROVIDER_CURSOR, ProviderQuota, SOURCE_CURSOR_CONNECT,
    WINDOW_BILLING, Window, clamp_pct, set_source,
};
use session::{cursor_auth_path, default_cursor_token, write_cursor_auth_file};

pub const DEFAULT_CURSOR_PERIOD_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
pub const DEFAULT_CURSOR_AGG_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetAggregatedUsageEvents";
pub const DEFAULT_CURSOR_PROFILE_URL: &str = "https://api2.cursor.sh/auth/full_stripe_profile";
pub const DEFAULT_CURSOR_ME_URL: &str = "https://api2.cursor.sh/auth/me";
pub const DEFAULT_CURSOR_WEB_ME_URL: &str = "https://cursor.com/api/auth/me";
pub const DEFAULT_CURSOR_GET_ME_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetMe";

fn quota() -> ProviderQuota {
    ProviderQuota {
        id: PROVIDER_CURSOR.into(),
        name: "Cursor".into(),
        ..Default::default()
    }
}

pub(crate) async fn fetch_cursor(ctx: &Ctx, f: &Fetcher) -> ProviderQuota {
    if f.cursor_connecting() {
        return cursor_connecting_quota();
    }
    let mut out = quota();
    let tok = match &f.cursor_token {
        Some(h) => h(),
        None => default_cursor_token(f.data_dir().as_deref()),
    };
    let mut tok = match tok {
        Ok(t) if !t.access.is_empty() => t,
        _ => {
            out.error = "No Cursor usage session".into();
            out.hint = cursor_hint_no_session().into();
            return out;
        }
    };
    if let Some(refreshed) = f.maybe_refresh_cursor(ctx, &tok).await {
        tok = refreshed;
    }
    if tok.source == SOURCE_CURSOR_CONNECT {
        tok = f.ensure_cursor_api_key(ctx, tok).await;
    }
    set_source(&mut out, &tok.source);
    out.email.clone_from(&tok.email);

    let res = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_period_url, DEFAULT_CURSOR_PERIOD_URL),
        Some(b"{}".to_vec()),
        &cursor_api_headers(&tok.access),
    )
    .await;
    let (status, raw) = match res {
        Ok(r) => r,
        Err(e) => {
            out.error = "Could not reach Cursor".into();
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
        out.error = "Cursor session expired".into();
        out.hint = "Reconnect in Settings.".into();
        return out;
    }
    if status >= 300 {
        out.error = "Cursor usage unavailable".into();
        out.hint = http_err(status, &raw);
        return out;
    }
    let Ok(period) = parse_cursor_period(&raw) else {
        out.error = "Cursor usage response was unreadable".into();
        return out;
    };
    let mut q = period.quota;
    set_source(&mut q, &tok.source);
    if q.email.is_empty() {
        q.email.clone_from(&tok.email);
    }
    if q.email.is_empty() {
        q.email = fetch_cursor_email(ctx, f, &tok.access).await;
    }
    let body = format!(
        "{{\"endDate\":{},\"startDate\":{}}}",
        serde_json::to_string(&period.end).unwrap_or_default(),
        serde_json::to_string(&period.start).unwrap_or_default()
    );
    if let Ok((st, araw)) = do_json(
        ctx,
        &f.http,
        Method::POST,
        f.url(&f.cursor_agg_url, DEFAULT_CURSOR_AGG_URL),
        Some(body.into_bytes()),
        &cursor_api_headers(&tok.access),
    )
    .await
        && st < 300
    {
        q.models = parse_cursor_models(&araw, period.auto_pct, period.api_pct, period.total_pct);
    }
    if !q.email.is_empty() && tok.source == SOURCE_CURSOR_CONNECT && tok.email.is_empty() {
        tok.email.clone_from(&q.email);
        if let Some(dir) = f.data_dir() {
            let _ = write_cursor_auth_file(&cursor_auth_path(&dir), &tok);
        }
    }
    q
}

pub(crate) fn cursor_api_headers(access: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {access}")),
        ("Content-Type", "application/json".into()),
        ("Accept", "application/json".into()),
        ("Connect-Protocol-Version", "1".into()),
        ("Origin", "https://cursor.com".into()),
    ]
}

impl Fetcher {
    /// Rotates a Connect session JWT a week before it expires.
    async fn maybe_refresh_cursor(&self, ctx: &Ctx, tok: &CursorTok) -> Option<CursorTok> {
        if tok.source != SOURCE_CURSOR_CONNECT
            || tok.refresh.is_empty()
            || !jwt_needs_refresh(&tok.access)
        {
            return None;
        }
        let mut next = oauth::refresh_cursor_token(ctx, self, &tok.refresh)
            .await
            .ok()?;
        if next.access.is_empty() {
            return None;
        }
        if next.email.is_empty() {
            next.email.clone_from(&tok.email);
        }
        if next.api_key.is_empty() {
            next.api_key.clone_from(&tok.api_key);
            next.api_key_expires_at_ms = tok.api_key_expires_at_ms;
            next.api_key_id = tok.api_key_id;
        }
        if let Some(dir) = self.data_dir() {
            let _ = write_cursor_auth_file(&cursor_auth_path(&dir), &next);
        }
        Some(next)
    }
}

/// The current billing period and its pool percentages.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CursorPeriod {
    pub quota: ProviderQuota,
    pub start: String,
    pub end: String,
    pub auto_pct: f64,
    pub api_pct: f64,
    pub total_pct: f64,
}

pub(crate) fn parse_cursor_period(raw: &[u8]) -> Result<CursorPeriod, String> {
    let m = decode_map(raw).map_err(|e| format!("cursor period: {e}"))?;
    let mut q = quota();
    q.email = email_from_cursor_json(Some(&m));
    let start = first_string(&m, &["billingCycleStart", "billing_cycle_start"]);
    let end = first_string(&m, &["billingCycleEnd", "billing_cycle_end"]);
    let plan = map_of(m.get("planUsage"))
        .or_else(|| map_of(m.get("plan_usage")))
        .ok_or("cursor period: no planUsage")?;
    let auto = clamp_pct_field(plan, &["autoPercentUsed", "auto_percent_used"]);
    let api = clamp_pct_field(plan, &["apiPercentUsed", "api_percent_used"]);
    let total = clamp_pct_field(plan, &["totalPercentUsed", "total_percent_used"]);
    // The Included Usage table is two pools (Cursor Models vs Other Models).
    // totalPercentUsed is a blend that undershoots the dashboard headline;
    // planUsage.limit is the Other Models API-dollar cap, not the Cursor pool.
    let mut pct = None;
    if auto.is_some() || api.is_some() {
        let a = auto.unwrap_or(0.0);
        pct = Some(match api {
            Some(p) if p > a => p,
            _ => a,
        });
    } else if let Some(t) = total {
        pct = Some(t);
    }
    if pct.is_none() {
        let limit = as_float(first_any(plan, &["limit"]));
        let used = as_float(first_any(
            plan,
            &[
                "totalSpend",
                "includedSpend",
                "total_spend",
                "included_spend",
            ],
        ));
        if limit > 0.0 {
            pct = Some(clamp_pct(used / limit * 100.0));
        }
    }
    let pct = pct.ok_or("cursor period: no usage percent")?;
    q.windows = vec![Window {
        id: WINDOW_BILLING.into(),
        label: "Included usage".into(),
        used_pct: pct,
        resets_at: parse_resets_at(Some(&serde_json::Value::String(end.clone()))),
        detail: String::new(),
    }];
    Ok(CursorPeriod {
        quota: q,
        start,
        end,
        auto_pct: auto.unwrap_or(0.0),
        api_pct: api.unwrap_or(0.0),
        total_pct: total.unwrap_or(0.0),
    })
}

/// Per-model rows from aggregated usage events, deduped by name, sorted by
/// spend then tokens then name, each scaled to its pool's percentage.
pub(crate) fn parse_cursor_models(
    raw: &[u8],
    auto_pct: f64,
    api_pct: f64,
    total_pct: f64,
) -> Vec<ModelRow> {
    let Ok(m) = decode_map(raw) else {
        return Vec::new();
    };
    let mut rows = slice_of(m.get("aggregations"));
    if rows.is_empty() {
        rows = slice_of(m.get("aggregation"));
    }
    let mut items: Vec<(ModelRow, f64)> = Vec::new();
    let mut by_name = std::collections::HashMap::new();
    let (mut cursor_cents, mut other_cents) = (0.0, 0.0);
    for row in rows {
        let Some(rm) = row.as_object() else { continue };
        let name = first_string(rm, &["modelIntent", "model", "name"]);
        if name.is_empty() {
            continue;
        }
        let mut tokens = [
            "inputTokens",
            "outputTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
        ]
        .iter()
        .zip([
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
        ])
        .map(|(a, b)| as_int64(first_any(rm, &[a, b])))
        .sum::<i64>();
        if tokens == 0 {
            tokens = as_int64(first_any(rm, &["totalTokens", "total_tokens"]));
        }
        let cents = as_float(first_any(rm, &["totalCents", "total_cents"]));
        if let Some(&i) = by_name.get(&name) {
            let (row, c): &mut (ModelRow, f64) = &mut items[i];
            row.tokens += tokens;
            *c += cents;
            if row.bucket == BUCKET_OTHER {
                other_cents += cents;
            } else {
                cursor_cents += cents;
            }
            continue;
        }
        let bucket = cursor_model_bucket(&name, rm);
        if bucket == BUCKET_OTHER {
            other_cents += cents;
        } else {
            cursor_cents += cents;
        }
        by_name.insert(name.clone(), items.len());
        items.push((
            ModelRow {
                name,
                tokens,
                used_pct: 0.0,
                bucket: bucket.into(),
            },
            cents,
        ));
    }
    items.sort_by(|(a, ac), (b, bc)| {
        bc.partial_cmp(ac)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.tokens.cmp(&a.tokens))
            .then(a.name.cmp(&b.name))
    });
    let cursor_scale = if auto_pct == 0.0 { total_pct } else { auto_pct };
    items
        .into_iter()
        .map(|(mut row, cents)| {
            let (sum, scale) = if row.bucket == BUCKET_OTHER {
                (other_cents, api_pct)
            } else {
                (cursor_cents, cursor_scale)
            };
            if sum > 0.0 && scale > 0.0 && cents > 0.0 {
                row.used_pct = clamp_pct(cents / sum * scale);
            }
            row
        })
        .collect()
}

fn cursor_model_bucket(name: &str, rm: &Obj) -> &'static str {
    match as_int64(rm.get("tier")) {
        1 => return BUCKET_OTHER,
        2 => return BUCKET_CURSOR,
        _ => {}
    }
    let n = name.trim().to_lowercase();
    if n.starts_with("cursor-")
        || n.starts_with("composer-")
        || n.starts_with("grok-")
        || n == "auto"
    {
        BUCKET_CURSOR
    } else {
        BUCKET_OTHER
    }
}

/// The account email: the JWT, then the dashboard `me` (cookie is
/// `sub::access`, not a Bearer token), then the REST profile and `me`, then
/// `GetMe`.
pub(crate) async fn fetch_cursor_email(ctx: &Ctx, f: &Fetcher, access: &str) -> String {
    let e = email_from_jwt(access);
    if !e.is_empty() {
        return e;
    }
    let try_one =
        async |method: Method, url: &str, body: Option<Vec<u8>>, headers: Vec<(&str, String)>| {
            if url.is_empty() {
                return String::new();
            }
            match do_json(ctx, &f.http, method, url, body, &headers).await {
                Ok((status, raw)) if status < 300 => decode_map(&raw)
                    .map(|m| email_from_cursor_json(Some(&m)))
                    .unwrap_or_default(),
                _ => String::new(),
            }
        };
    let sub = jwt_claim(access, "sub");
    if !sub.is_empty() {
        let web = vec![
            ("Accept", "application/json".to_owned()),
            ("Origin", "https://cursor.com".to_owned()),
            (
                "Cookie",
                format!("WorkosCursorSessionToken={sub}::{access}"),
            ),
        ];
        let e = try_one(
            Method::GET,
            f.url(&f.cursor_web_me_url, DEFAULT_CURSOR_WEB_ME_URL),
            None,
            web,
        )
        .await;
        if !e.is_empty() {
            return e;
        }
    }
    let rest = || {
        vec![
            ("Authorization", format!("Bearer {access}")),
            ("Accept", "application/json".to_owned()),
            ("Origin", "https://cursor.com".to_owned()),
        ]
    };
    for url in [
        f.url(&f.cursor_profile_url, DEFAULT_CURSOR_PROFILE_URL),
        f.url(&f.cursor_me_url, DEFAULT_CURSOR_ME_URL),
    ] {
        let e = try_one(Method::GET, url, None, rest()).await;
        if !e.is_empty() {
            return e;
        }
    }
    try_one(
        Method::POST,
        f.url(&f.cursor_get_me_url, DEFAULT_CURSOR_GET_ME_URL),
        Some(b"{}".to_vec()),
        cursor_api_headers(access),
    )
    .await
}
