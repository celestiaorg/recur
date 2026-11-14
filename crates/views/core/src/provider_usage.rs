//! Which providers get a usage chip, their session rings, model buckets,
//! reset and percent formatting, and the error / disconnect checks Settings shares with the
//! sidebar's meter.

use gpui_kit::App;
use recur_services::api::{ProviderQuota, Snapshot};
use recur_ui::Theme;
use recur_ui::color::Color;

/// Extra / credit balances are overage, not the session chip.
pub fn is_chip_window(w: &recur_services::api::Window) -> bool {
    w.id != "extra" && w.used_pct.is_finite()
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChipRing {
    pub id: String,
    pub label: String,
    pub pct: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderChip {
    pub id: String,
    pub name: String,
    pub rings: Vec<ChipRing>,
    pub headline: f64,
}

const CHIP_ORDER: [&str; 4] = ["claude-code", "cursor", "codex", "chatgpt"];

fn ring_rank(id: &str) -> u8 {
    match id {
        "five_hour" => 0,
        "seven_day" => 1,
        _ if id.starts_with("seven_day_") => 2,
        "billing" => 3,
        _ => 4,
    }
}

/// Cursor usage is Recur Connect only: a row with no account is hidden.
pub fn cursor_usage_visible(p: &ProviderQuota) -> bool {
    p.id != "cursor" || !p.email.is_empty() || !p.plan.is_empty() || p.source_id == "cursor_connect"
}

/// Providers off in Settings stay out of the meter and popover.
pub fn usage_provider_on(id: &str, disabled: &[String]) -> bool {
    !id.is_empty() && !disabled.iter().any(|d| d == id)
}

pub fn providers(s: Option<&Snapshot>) -> &[ProviderQuota] {
    s.and_then(|s| s.providers.as_deref()).unwrap_or(&[])
}

/// `providerChips`: one chip per provider that has session quota.
pub fn provider_chips(s: Option<&Snapshot>, disabled: &[String]) -> Vec<ProviderChip> {
    let mut out: Vec<ProviderChip> = providers(s)
        .iter()
        .filter(|p| usage_provider_on(&p.id, disabled) && cursor_usage_visible(p))
        .filter_map(|p| {
            let mut rings: Vec<ChipRing> = p
                .windows
                .iter()
                .filter(|w| is_chip_window(w))
                .map(|w| ChipRing {
                    id: w.id.clone(),
                    label: w.label.clone(),
                    pct: w.used_pct,
                })
                .collect();
            rings.sort_by(|a, b| {
                ring_rank(&a.id)
                    .cmp(&ring_rank(&b.id))
                    .then_with(|| a.id.cmp(&b.id))
            });
            (!rings.is_empty()).then(|| ProviderChip {
                id: p.id.clone(),
                name: p.name.clone(),
                headline: rings
                    .iter()
                    .map(|r| r.pct)
                    .fold(f64::NEG_INFINITY, f64::max),
                rings,
            })
        })
        .collect();
    let rank = |id: &str| CHIP_ORDER.iter().position(|x| *x == id).unwrap_or(99);
    out.sort_by_key(|c| rank(&c.id));
    out
}

/// A row with nothing to show, like a signed-out ChatGPT account, is hidden.
pub fn usage_row_has_data(p: &ProviderQuota) -> bool {
    !p.windows.is_empty()
        || !p.error.is_empty()
        || p.connecting
        || !p.hint.is_empty()
        || !p.source_id.is_empty()
}

pub fn snapshot_has_data(s: Option<&Snapshot>, disabled: &[String]) -> bool {
    providers(s)
        .iter()
        .any(|p| usage_provider_on(&p.id, disabled) && usage_row_has_data(p))
}

/// True when an enabled Claude row can recover through `claude auth status`.
pub fn snapshot_has_provider_error(s: Option<&Snapshot>, disabled: &[String]) -> bool {
    providers(s).iter().any(|p| {
        usage_provider_on(&p.id, disabled)
            && p.id == "claude-code"
            && (p.error == "Claude Code login expired" || p.error == "Not signed in to Claude Code")
    })
}

/// `formatResetIn`.
pub fn format_reset_in(ms: i64, now: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let sec = ((ms - now) as f64 / 1000.).round().max(0.) as i64;
    if sec < 60 {
        "resets now".into()
    } else if sec < 3600 {
        format!("resets in {}m", sec / 60)
    } else if sec < 86400 {
        format!("resets in {}h", sec / 3600)
    } else {
        format!("resets in {}d", sec / 86400)
    }
}

/// `formatUsagePct`: one decimal like the dashboard.
pub fn format_usage_pct(n: f64) -> String {
    if !n.is_finite() {
        return "0.0%".into();
    }
    format!("{:.1}%", (n * 10.).round() / 10.)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelBucket {
    pub id: &'static str,
    pub label: &'static str,
    pub tokens: i64,
    pub used_pct: f64,
    pub models: Vec<recur_services::api::ModelRow>,
}

/// `modelBuckets`: Cursor's aggregation grouped the way the dashboard does.
pub fn model_buckets(models: &[recur_services::api::ModelRow]) -> [ModelBucket; 2] {
    let mut cursor = ModelBucket {
        id: "cursor",
        label: "Cursor Models",
        tokens: 0,
        used_pct: 0.,
        models: Vec::new(),
    };
    let mut other = ModelBucket {
        id: "other",
        label: "Other Models",
        tokens: 0,
        used_pct: 0.,
        models: Vec::new(),
    };
    for m in models {
        let b = if m.bucket == "other" {
            &mut other
        } else {
            &mut cursor
        };
        b.tokens += m.tokens;
        b.used_pct += m.used_pct;
        b.models.push(m.clone());
    }
    [cursor, other]
}

/// `ringColor`.
pub fn ring_color(pct: f64, cx: &App) -> Color {
    let t = Theme::of(cx).tokens;
    if pct >= 90. {
        t.coral
    } else if pct >= 70. {
        t.amber
    } else {
        t.muted
    }
}

pub fn can_disconnect_cursor(s: Option<&Snapshot>) -> bool {
    providers(s)
        .iter()
        .find(|p| p.id == "cursor")
        .is_some_and(|p| p.source_id == "cursor_connect")
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::{ModelRow, Window as Win};

    fn w(id: &str, label: &str, pct: f64) -> Win {
        Win {
            id: id.into(),
            label: label.into(),
            used_pct: pct,
            ..Win::default()
        }
    }

    fn snap(providers: Vec<ProviderQuota>) -> Snapshot {
        Snapshot {
            providers: Some(providers),
            ..Snapshot::default()
        }
    }

    fn p(id: &str, f: impl FnOnce(&mut ProviderQuota)) -> ProviderQuota {
        let mut q = ProviderQuota {
            id: id.into(),
            name: id.into(),
            ..ProviderQuota::default()
        };
        f(&mut q);
        q
    }

    #[test]
    fn chips_stack_session_rings_and_skip_extra() {
        let s = snap(vec![
            p("claude-code", |q| {
                q.windows = vec![
                    w("five_hour", "5-hour", 0.),
                    w("seven_day", "7-day", 34.),
                    w("seven_day_fable", "Fable (7-day)", 12.),
                    w("extra", "Extra", 100.),
                ]
            }),
            p("cursor", |q| {
                q.email = "dev@example.com".into();
                q.windows = vec![w("billing", "Included usage", 27.4)];
            }),
        ]);
        let chips = provider_chips(Some(&s), &[]);
        assert_eq!(
            chips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["claude-code", "cursor"]
        );
        assert_eq!(
            chips[0]
                .rings
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["five_hour", "seven_day", "seven_day_fable"]
        );
        assert_eq!(chips[0].headline, 34.);
        assert_eq!(chips[1].headline, 27.4);
        let codex = snap(vec![p("codex", |q| {
            q.windows = vec![
                w("five_hour", "5-hour", 100.),
                w("seven_day", "7-day", 16.),
                w("billing", "Monthly", 100.),
                w("extra", "Credits", 0.),
            ]
        })]);
        let chips = provider_chips(Some(&codex), &[]);
        assert_eq!(chips[0].rings.len(), 3);
        assert_eq!(chips[0].rings[2].id, "billing");
        let off = provider_chips(Some(&s), &["cursor".into()]);
        assert_eq!(off.len(), 1);
    }

    #[test]
    fn chatgpt_chip_sorts_after_codex_and_respects_disabled() {
        let s = snap(
            ["chatgpt", "codex", "claude-code"]
                .map(|id| p(id, |q| q.windows = vec![w("five_hour", "5-hour", 10.)]))
                .into(),
        );
        let ids = |d: &[String]| {
            provider_chips(Some(&s), d)
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&[]), ["claude-code", "codex", "chatgpt"]);
        assert_eq!(ids(&["chatgpt".into()]), ["claude-code", "codex"]);
    }

    #[test]
    fn cursor_visibility_and_data() {
        assert!(!cursor_usage_visible(&p("cursor", |_| {})));
        assert!(cursor_usage_visible(
            &p("cursor", |q| q.email = "a@b.c".into())
        ));
        assert!(cursor_usage_visible(
            &p("cursor", |q| q.source_id = "cursor_connect".into())
        ));
        assert!(cursor_usage_visible(&p("claude-code", |_| {})));
        assert!(snapshot_has_data(
            Some(&snap(vec![p("cursor", |q| q.error = "no".into())])),
            &[]
        ));
        let hinted = snap(vec![p("cursor", |q| {
            q.source_id = "cursor_connect".into();
            q.hint = "off".into();
        })]);
        assert!(snapshot_has_data(Some(&hinted), &[]));
        assert!(!snapshot_has_data(Some(&snap(vec![])), &[]));
        assert!(!snapshot_has_data(Some(&hinted), &["cursor".into()]));
        assert!(!usage_row_has_data(&p("chatgpt", |_| {})));
    }

    #[test]
    fn provider_error() {
        assert!(!snapshot_has_provider_error(None, &[]));
        let err = |id: &str, e: &str| snap(vec![p(id, |q| q.error = e.into())]);
        assert!(snapshot_has_provider_error(
            Some(&err("claude-code", "Claude Code login expired")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("cursor", "No Cursor usage session")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("codex", "Not signed in to Codex")),
            &[]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("claude-code", "Claude Code login expired")),
            &["claude-code".into()]
        ));
        assert!(!snapshot_has_provider_error(
            Some(&err("claude-code", "Usage endpoint is rate limited")),
            &[]
        ));
    }

    #[test]
    fn formatting() {
        let now = 1_787_227_200_000;
        assert_eq!(format_reset_in(now + 90_000, now), "resets in 1m");
        assert_eq!(format_reset_in(now + 3 * 3_600_000, now), "resets in 3h");
        assert_eq!(format_usage_pct(27.14), "27.1%");
        assert_eq!(format_usage_pct(0.), "0.0%");
        assert!(can_disconnect_cursor(Some(&snap(vec![p("cursor", |q| {
            q.source_id = "cursor_connect".into()
        })]))));
        assert!(!can_disconnect_cursor(Some(&snap(vec![p(
            "cursor",
            |_| {}
        )]))));
    }

    #[test]
    fn buckets() {
        let m = |name: &str, tokens: i64, pct: f64, bucket: &str| ModelRow {
            name: name.into(),
            tokens,
            used_pct: pct,
            bucket: bucket.into(),
        };
        let [cursor, other] = model_buckets(&[
            m("cursor-grok-4.6-high-fast", 590_200_000, 22.7, "cursor"),
            m("claude-4.6-opus", 12_000, 1.1, "other"),
            m("composer-2.5-fast", 4_300_000, 0.1, "cursor"),
        ]);
        assert_eq!(cursor.label, "Cursor Models");
        assert_eq!(cursor.tokens, 594_500_000);
        assert!((cursor.used_pct - 22.8).abs() < 1e-9);
        assert_eq!(cursor.models.len(), 2);
        assert_eq!(other.label, "Other Models");
        assert_eq!(other.tokens, 12_000);
    }
}
