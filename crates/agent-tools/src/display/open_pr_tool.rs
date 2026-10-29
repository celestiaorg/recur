//! mcp__recur__open_pr chat cards.

use serde_json::Value;

use super::{contains_ci, is_tool, js_trim, starts_with_ci};

pub fn is_open_pr_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "open_pr")
}

/// Where the PR lives, as far as the result text says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrCardSource {
    GitHub,
    Local,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenPrCard {
    pub title: String,
    pub draft: bool,
    pub number: u64,
    pub source: PrCardSource,
    pub promoted: bool,
    pub pushed: bool,
    pub already: bool,
    pub failed: bool,
    pub pending: bool,
}

/// Title and draft from the open_pr args.
pub fn parse_open_pr_input(raw: &Value) -> (String, bool) {
    let Value::Object(o) = raw else {
        return (String::new(), false);
    };
    let title = match o.get("title") {
        Some(Value::String(s)) => js_trim(s).to_string(),
        _ => String::new(),
    };
    (title, o.get("draft") == Some(&Value::Bool(true)))
}

/// The first `#<digits>` in the text.
// simplify: digits beyond u64 read as 0; JavaScript would keep a float.
fn pr_number(r: &str) -> u64 {
    let Some(i) = r.find('#') else { return 0 };
    let rest = &r[i + 1..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        // JavaScript's /#(\d+)/ keeps searching after a bare '#'.
        return pr_number(rest);
    }
    rest[..end].parse().unwrap_or(0)
}

pub fn parse_open_pr_card(raw: &Value, result: &str, pending: bool, error: bool) -> OpenPrCard {
    let (title, draft) = parse_open_pr_input(raw);
    let r = js_trim(result);
    let failed = error || (!pending && starts_with_ci(r, "open_pr:"));
    // Local fallback copy mentions "GitHub create failed": check Local first.
    let source = if contains_ci(r, "Opened Local PR") || contains_ci(r, "already open (Local)") {
        PrCardSource::Local
    } else if contains_ci(r, "GitHub") {
        PrCardSource::GitHub
    } else if contains_ci(r, "Local") {
        PrCardSource::Local
    } else {
        PrCardSource::Unknown
    };
    OpenPrCard {
        title,
        draft: draft || contains_ci(r, "GitHub draft PR"),
        number: pr_number(r),
        source,
        promoted: contains_ci(r, "promoted from Local"),
        pushed: contains_ci(r, "Pushed new commits"),
        already: contains_ci(r, "already open"),
        failed,
        pending,
    }
}

/// The collapsed-row title: never the args JSON.
pub fn open_pr_label(card: &OpenPrCard) -> String {
    if card.pending {
        if card.title.is_empty() {
            return "Opening pull request".into();
        }
        return format!("Opening {}", card.title);
    }
    if card.failed {
        return "Open PR failed".into();
    }
    let n = if card.number > 0 {
        format!(" #{}", card.number)
    } else {
        String::new()
    };
    if card.pushed {
        return format!("Pushed to PR{n}");
    }
    if card.already {
        return format!("PR{n} already open");
    }
    match card.source {
        PrCardSource::GitHub if card.draft => format!("Opened GitHub draft PR{n}"),
        PrCardSource::GitHub => format!("Opened GitHub PR{n}"),
        PrCardSource::Local => format!("Opened Local PR{n}"),
        PrCardSource::Unknown if n.is_empty() => "Opened pull request".into(),
        PrCardSource::Unknown => format!("Opened PR{n}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn matches_recur_mcp_and_bare_names() {
        assert!(is_open_pr_tool(Some("mcp__recur__open_pr")));
        assert!(is_open_pr_tool(Some("open_pr")));
        assert!(!is_open_pr_tool(Some("mcp__recur__rename_workspace")));
        assert!(!is_open_pr_tool(None));
    }

    fn args() -> Value {
        json!({
            "title": "Align screening allow cache TTL to 24 hours",
            "body": "## Summary\nSet ALLOW_TTL to 86400.",
            "draft": true,
        })
    }

    #[test]
    fn reads_title_and_draft_from_args_number_from_result() {
        let card = parse_open_pr_card(
            &args(),
            "Opened GitHub draft PR #112 (promoted from Local).",
            false,
            false,
        );
        assert_eq!(card.title, "Align screening allow cache TTL to 24 hours");
        assert!(card.draft);
        assert_eq!(card.number, 112);
        assert_eq!(card.source, PrCardSource::GitHub);
        assert!(card.promoted && !card.failed);
        assert_eq!(open_pr_label(&card), "Opened GitHub draft PR #112");
    }

    #[test]
    fn does_not_treat_the_markdown_body_as_the_label() {
        let label = open_pr_label(&parse_open_pr_card(
            &args(),
            "Opened GitHub draft PR #112 (promoted from Local).",
            false,
            false,
        ));
        assert!(!label.contains("Summary") && !label.contains("ALLOW_TTL") && !label.contains('{'));
    }

    #[test]
    fn labels_a_local_fallback_and_a_later_push() {
        let t = json!({"title": "Fix"});
        assert_eq!(
            open_pr_label(&parse_open_pr_card(&t, "Opened Local PR #4.", false, false)),
            "Opened Local PR #4"
        );
        assert_eq!(
            open_pr_label(&parse_open_pr_card(
                &t,
                "Pushed new commits to GitHub PR #91 (description unchanged).",
                false,
                false
            )),
            "Pushed to PR #91"
        );
    }

    #[test]
    fn local_fallback_mentioning_github_stays_local() {
        let card = parse_open_pr_card(
            &json!({"title": "Add client-side acme.dev API"}),
            "Opened Local PR #164 (GitHub create failed: gh pr create failed: HTTP 401 — retry Open PR).",
            false,
            false,
        );
        assert_eq!(card.source, PrCardSource::Local);
        assert_eq!(card.number, 164);
        assert_eq!(open_pr_label(&card), "Opened Local PR #164");
        let already = parse_open_pr_card(
            &json!({"title": "Retry"}),
            "PR #164 is already open (Local). GitHub create failed (git push failed) — retry Open PR.",
            false,
            false,
        );
        assert_eq!(already.source, PrCardSource::Local);
        assert_eq!(open_pr_label(&already), "PR #164 already open");
    }

    #[test]
    fn labels_pending_and_engine_errors() {
        assert_eq!(
            open_pr_label(&parse_open_pr_card(&args(), "", true, false)),
            "Opening Align screening allow cache TTL to 24 hours"
        );
        let fail = parse_open_pr_card(
            &args(),
            "open_pr: the main working copy can't open a PR",
            false,
            false,
        );
        assert!(fail.failed);
        assert_eq!(open_pr_label(&fail), "Open PR failed");
    }
}
