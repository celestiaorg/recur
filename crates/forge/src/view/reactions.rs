//! Reaction pills on conversation rows (`lib/comment-reactions.ts`). The
//! content mapping and toggling live in `recur_domain::reaction`.

use std::collections::HashMap;

use recur_domain::{PrComment, PrReaction, normalize_reaction_content};

/// The reaction picker: content, icon name, accessible label.
pub const REACTIONS: [(&str, &str, &str); 8] = [
    ("+1", "thumbs-up", "thumbs up"),
    ("-1", "thumbs-down", "thumbs down"),
    ("laugh", "face-grinning", "laugh"),
    ("hooray", "party-popper", "hooray"),
    ("confused", "face-slightly-frowning", "confused"),
    ("heart", "heart", "heart"),
    ("rocket", "rocket", "rocket"),
    ("eyes", "eye", "eyes"),
];

fn entry(content: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let n = normalize_reaction_content(content)?;
    REACTIONS.iter().find(|(c, _, _)| *c == n)
}

pub fn reaction_icon(content: &str) -> Option<&'static str> {
    entry(content).map(|(_, icon, _)| *icon)
}

pub fn reaction_label(content: &str) -> String {
    match entry(content) {
        Some((_, _, label)) => (*label).to_owned(),
        None => normalize_reaction_content(content)
            .unwrap_or_default()
            .to_owned(),
    }
}

/// Known reactions with a count, normalized and in picker order.
pub fn visible_reactions(rs: &[PrReaction]) -> Vec<PrReaction> {
    let mut out: Vec<PrReaction> = rs
        .iter()
        .filter(|r| r.count > 0)
        .filter_map(|r| {
            let content = normalize_reaction_content(&r.content)?;
            Some(PrReaction {
                content: content.to_owned(),
                ..r.clone()
            })
        })
        .collect();
    out.sort_by_key(|r| REACTIONS.iter().position(|(c, _, _)| *c == r.content));
    out
}

/// Sets one row's reactions.
pub fn apply_comment_reactions(comments: &mut [PrComment], id: &str, reactions: &[PrReaction]) {
    if id.is_empty() {
        return;
    }
    for c in comments.iter_mut().filter(|c| c.id == id) {
        c.reactions = reactions.to_vec();
    }
}

/// Re-applies in-flight toggles so a stale refetch cannot flip pills back.
pub fn apply_reaction_holds(comments: &mut [PrComment], holds: &HashMap<String, Vec<PrReaction>>) {
    for (id, rs) in holds {
        apply_comment_reactions(comments, id, rs);
    }
}

#[cfg(test)]
mod tests {
    use recur_domain::{reaction_mine, toggle_reactions};

    use super::*;

    fn r(content: &str, count: i64, mine: bool) -> PrReaction {
        PrReaction {
            content: content.into(),
            count,
            mine,
        }
    }

    #[test]
    fn icons_and_visible_order() {
        assert_eq!(reaction_icon("heart"), Some("heart"));
        assert_eq!(reaction_icon("THUMBS_UP"), Some("thumbs-up"));
        assert_eq!(reaction_icon("wave"), None);
        assert_eq!(reaction_label("+1"), "thumbs up");
        assert_eq!(
            visible_reactions(&[
                r("rocket", 1, false),
                r("+1", 0, false),
                r("heart", 2, false),
                r("wave", 3, false)
            ]),
            [r("heart", 2, false), r("rocket", 1, false)]
        );
    }

    #[test]
    fn toggle_matches_visible() {
        let start = [r("heart", 1, true), r("+1", 2, false)];
        assert_eq!(
            toggle_reactions(&start, "rocket"),
            [r("+1", 2, false), r("heart", 1, true), r("rocket", 1, true)]
        );
        assert_eq!(toggle_reactions(&start, "heart"), [r("+1", 2, false)]);
        assert_eq!(
            visible_reactions(&toggle_reactions(&start, "wave")),
            visible_reactions(&start)
        );
    }

    #[test]
    fn holds_reapply_over_stale_rows() {
        let mut stale = vec![
            PrComment {
                id: "c1".into(),
                reactions: vec![r("+1", 2, false)],
                ..Default::default()
            },
            PrComment {
                id: "c2".into(),
                ..Default::default()
            },
        ];
        let held = vec![r("+1", 3, true)];
        apply_reaction_holds(
            &mut stale,
            &HashMap::from([("c1".to_owned(), held.clone())]),
        );
        assert_eq!(stale[0].reactions, held);
        assert!(stale[1].reactions.is_empty());
        assert!(reaction_mine(&held, "+1"));
    }
}
