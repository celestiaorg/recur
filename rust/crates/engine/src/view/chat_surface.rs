//! `lib/chat-surface.ts`: the looking-at surface the UI stamps onto a chat
//! wire (never the bubble) so the engine knows whether the operator is on a
//! workspace, a Local PR or a forge PR. The engine side is
//! `chat::surface`.

use recur_domain::Pr;

use crate::chat::surface::{
    CHAT_SURFACE_CLOSE, CHAT_SURFACE_GITHUB_PR, CHAT_SURFACE_LOCAL_PR, CHAT_SURFACE_OPEN,
    CHAT_SURFACE_WORKSPACE, ChatSurface,
};

/// The surface for the current view: PR detail of a forge PR (`gh`) is a
/// forge surface, of a Local PR a Local one; every other view is the
/// workspace.
pub fn chat_surface_for_view(view: &str, pr: Option<&Pr>) -> ChatSurface {
    let Some(pr) = pr.filter(|p| view == "prdetail" && !p.id.trim().is_empty()) else {
        return ChatSurface {
            surface: CHAT_SURFACE_WORKSPACE.into(),
            ..ChatSurface::default()
        };
    };
    let source = pr.source.as_str().to_lowercase();
    let forge = source == "gh" || source == "github";
    ChatSurface {
        surface: if forge {
            CHAT_SURFACE_GITHUB_PR
        } else {
            CHAT_SURFACE_LOCAL_PR
        }
        .into(),
        pr_id: pr.id.trim().to_owned(),
        pr_number: pr.number,
        pr_source: pr.source.as_str().to_owned(),
        pr_url: pr.url.clone(),
        pr_title: pr.title.clone(),
        pr_base: pr.base.clone(),
        pr_branch: pr.branch.clone(),
        host: false,
    }
}

/// Prefixes the chat wire with the marker (empty fields omitted, as
/// `JSON.stringify` drops `undefined`).
pub fn attach_chat_surface(text: &str, ctx: &ChatSurface) -> String {
    let payload = ChatSurface {
        host: false,
        ..ctx.clone()
    };
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    format!("{CHAT_SURFACE_OPEN}{json}{CHAT_SURFACE_CLOSE}\n{text}")
}

/// Strips a leading marker; unmarked and invalid wires come back as is.
pub fn peel_chat_surface(text: &str) -> String {
    let s = text.trim_start_matches([' ', '\t', '\r', '\n']);
    let Some(rest) = s.strip_prefix(CHAT_SURFACE_OPEN) else {
        return text.to_owned();
    };
    let Some(close) = rest.find(CHAT_SURFACE_CLOSE) else {
        return text.to_owned();
    };
    let valid = serde_json::from_str::<serde_json::Value>(rest[..close].trim())
        .ok()
        .and_then(|v| v.get("surface").and_then(|s| s.as_str()).map(str::to_owned))
        .is_some_and(|s| !s.trim().is_empty());
    if !valid {
        return text.to_owned();
    }
    rest[close + CHAT_SURFACE_CLOSE.len()..]
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use recur_domain::PrSource;

    use super::*;

    fn pr(id: &str, number: i64, source: PrSource) -> Pr {
        Pr {
            id: id.into(),
            number,
            source,
            ..Pr::default()
        }
    }

    #[test]
    fn workspace_surface_away_from_pr_detail() {
        let p = pr("gh-1", 1, PrSource::GitHub);
        assert_eq!(
            chat_surface_for_view("detail", Some(&p)).surface,
            CHAT_SURFACE_WORKSPACE
        );
        assert_eq!(
            chat_surface_for_view("inbox", None),
            ChatSurface {
                surface: "workspace".into(),
                ..ChatSurface::default()
            }
        );
    }

    #[test]
    fn github_pr_on_forge_pr_detail() {
        let p = Pr {
            url: "https://github.com/acme/r/pull/9".into(),
            title: "Imported".into(),
            base: "main".into(),
            branch: "feat".into(),
            ..pr("gh-acme-9", 9, PrSource::GitHub)
        };
        assert_eq!(
            chat_surface_for_view("prdetail", Some(&p)),
            ChatSurface {
                surface: "github_pr".into(),
                pr_id: "gh-acme-9".into(),
                pr_number: 9,
                pr_source: "gh".into(),
                pr_url: "https://github.com/acme/r/pull/9".into(),
                pr_title: "Imported".into(),
                pr_base: "main".into(),
                pr_branch: "feat".into(),
                host: false,
            }
        );
    }

    #[test]
    fn local_pr_on_local_pr_detail() {
        let p = Pr {
            title: "WIP".into(),
            ..pr("7", 7, PrSource::Local)
        };
        let s = chat_surface_for_view("prdetail", Some(&p));
        assert_eq!(s.surface, "local_pr");
        assert_eq!((s.pr_id.as_str(), s.pr_number), ("7", 7));
        assert_eq!(
            (s.pr_source.as_str(), s.pr_title.as_str()),
            ("local", "WIP")
        );
        assert!(s.pr_url.is_empty() && s.pr_base.is_empty() && s.pr_branch.is_empty());
    }

    #[test]
    fn attach_prefixes_a_peelable_marker() {
        let wire = attach_chat_surface(
            "please review",
            &ChatSurface {
                surface: "github_pr".into(),
                pr_number: 9,
                pr_id: "gh-9".into(),
                ..ChatSurface::default()
            },
        );
        assert!(wire.starts_with("⟦recur-surface⟧"));
        assert!(wire.ends_with("\nplease review"));
        assert!(wire.contains(r#""surface":"github_pr""#));
        assert!(wire.contains(r#""prNumber":9"#));
        assert_eq!(peel_chat_surface(&wire), "please review");
        // The engine peels the same wire.
        let (rest, surf) = crate::chat::surface::peel_chat_surface(&wire).unwrap();
        assert_eq!(
            (rest.as_str(), surf.pr_id.as_str()),
            ("please review", "gh-9")
        );
    }

    #[test]
    fn unmarked_and_invalid_wires_are_left_alone() {
        assert_eq!(peel_chat_surface("plain"), "plain");
        let bad = "⟦recur-surface⟧not-json⟦/recur-surface⟧\nhi";
        assert_eq!(peel_chat_surface(bad), bad);
    }
}
