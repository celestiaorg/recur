//! The looking-at surface shared with the agent and prompt actions (`lib/chat-surface.ts`).

use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSurface {
    /// `workspace`, `local_pr` or `github_pr`.
    pub surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pr_number: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_base: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_branch: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

const OPEN: &str = "⟦recur-surface⟧";
const CLOSE: &str = "⟦/recur-surface⟧";

/// `chatSurfaceForView`: PR detail with a PR is a `github_pr` or `local_pr` surface.
pub fn chat_surface_for_view(view: recur_view_core::ViewName, pr: Option<&recur_domain::Pr>) -> ChatSurface {
    let Some(pr) = pr.filter(|p| !p.id.trim().is_empty()) else {
        return ChatSurface {
            surface: "workspace".into(),
            ..Default::default()
        };
    };
    if view != recur_view_core::ViewName::PrDetail {
        return ChatSurface {
            surface: "workspace".into(),
            ..Default::default()
        };
    }
    let source = pr.source.as_str().to_lowercase();
    let github = source == "gh" || source == "github";
    ChatSurface {
        surface: if github { "github_pr" } else { "local_pr" }.into(),
        pr_id: pr.id.trim().to_string(),
        pr_number: pr.number,
        pr_source: pr.source.as_str().to_string(),
        pr_url: pr.url.clone(),
        pr_title: pr.title.clone(),
        pr_base: pr.base.clone(),
        pr_branch: pr.branch.clone(),
    }
}

/// `attachChatSurface`: prefix the wire (not the bubble) so the engine can peel the surface.
pub fn attach_chat_surface(text: &str, ctx: &ChatSurface) -> String {
    let payload = serde_json::to_string(ctx).unwrap_or_else(|_| "{}".into());
    format!("{OPEN}{payload}{CLOSE}\n{text}")
}

/// `peelChatSurface`: strip a leading marker; invalid markers stay.
pub fn peel_chat_surface(text: &str) -> &str {
    let s = text.trim_start_matches([' ', '\t', '\r', '\n']);
    let Some(rest) = s.strip_prefix(OPEN) else {
        return text;
    };
    let Some(close_at) = rest.find(CLOSE) else {
        return text;
    };
    let raw = rest[..close_at].trim();
    let ok = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("surface").and_then(|s| s.as_str()).map(|s| !s.trim().is_empty()))
        .unwrap_or(false);
    if !ok {
        return text;
    }
    rest[close_at + CLOSE.len()..].trim_start_matches([' ', '\t', '\r', '\n'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_view_core::ViewName;

    fn pr(f: impl FnOnce(&mut recur_domain::Pr)) -> recur_domain::Pr {
        let mut p = recur_domain::Pr::default();
        f(&mut p);
        p
    }

    // chat-surface.test.ts
    #[test]
    fn surface_for_view() {
        let gh = pr(|p| {
            p.id = "gh-1".into();
            p.source = "gh".into();
            p.number = 1;
        });
        assert_eq!(chat_surface_for_view(ViewName::Detail, Some(&gh)).surface, "workspace");
        assert_eq!(chat_surface_for_view(ViewName::Briefing, None).surface, "workspace");
        let imported = pr(|p| {
            p.id = "gh-acme-9".into();
            p.number = 9;
            p.source = "gh".into();
            p.url = "https://github.com/acme/r/pull/9".into();
            p.title = "Imported".into();
            p.base = "main".into();
            p.branch = "feat".into();
        });
        assert_eq!(
            chat_surface_for_view(ViewName::PrDetail, Some(&imported)),
            ChatSurface {
                surface: "github_pr".into(),
                pr_id: "gh-acme-9".into(),
                pr_number: 9,
                pr_source: "gh".into(),
                pr_url: "https://github.com/acme/r/pull/9".into(),
                pr_title: "Imported".into(),
                pr_base: "main".into(),
                pr_branch: "feat".into(),
            }
        );
        let local = pr(|p| {
            p.id = "7".into();
            p.number = 7;
            p.source = "local".into();
            p.title = "WIP".into();
        });
        let s = chat_surface_for_view(ViewName::PrDetail, Some(&local));
        assert_eq!(s.surface, "local_pr");
        assert_eq!((s.pr_id.as_str(), s.pr_title.as_str()), ("7", "WIP"));
    }

    #[test]
    fn attach_and_peel() {
        let wire = attach_chat_surface(
            "please review",
            &ChatSurface {
                surface: "github_pr".into(),
                pr_number: 9,
                pr_id: "gh-9".into(),
                ..Default::default()
            },
        );
        assert!(wire.starts_with("⟦recur-surface⟧"));
        assert!(wire.ends_with("\nplease review"));
        assert!(wire.contains("\"surface\":\"github_pr\""));
        assert!(wire.contains("\"prNumber\":9"));
        assert_eq!(peel_chat_surface(&wire), "please review");
        assert_eq!(peel_chat_surface("plain"), "plain");
        let bad = "⟦recur-surface⟧not-json⟦/recur-surface⟧\nhi";
        assert_eq!(peel_chat_surface(bad), bad);
    }
}
