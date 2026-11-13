//! GitHub conversation reactions: REST/GraphQL name mapping and rollups.

use crate::PrReaction;

/// Reaction contents in GitHub's picker order, with their GraphQL
/// ReactionContent names.
const REACTIONS: [(&str, &str); 8] = [
    ("+1", "THUMBS_UP"),
    ("-1", "THUMBS_DOWN"),
    ("laugh", "LAUGH"),
    ("hooray", "HOORAY"),
    ("confused", "CONFUSED"),
    ("heart", "HEART"),
    ("rocket", "ROCKET"),
    ("eyes", "EYES"),
];

fn alias(s: &str) -> Option<&'static str> {
    REACTIONS
        .iter()
        .find(|(rest, gql)| s == *rest || s == *gql || s == rest.to_uppercase())
        .map(|(rest, _)| *rest)
}

/// Maps a GitHub REST or GraphQL reaction name to the REST content string
/// (+1, laugh, …). Lookup tries the trimmed name as given, upper-cased, then
/// lower-cased.
pub fn normalize_reaction_content(s: &str) -> Option<&'static str> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    alias(s)
        .or_else(|| alias(&s.to_uppercase()))
        .or_else(|| alias(&s.to_lowercase()))
}

/// The ReactionContent enum value for addReaction/removeReaction.
pub fn graphql_reaction_content(s: &str) -> Option<&'static str> {
    let n = normalize_reaction_content(s)?;
    REACTIONS
        .iter()
        .find(|(rest, _)| *rest == n)
        .map(|(_, gql)| *gql)
}

/// Adds or removes the viewer's reaction of that content. Unknown content
/// leaves the rollup unchanged.
pub fn toggle_reactions(rs: &[PrReaction], content: &str) -> Vec<PrReaction> {
    let Some(content) = normalize_reaction_content(content) else {
        return rs.to_vec();
    };
    let mut out = Vec::with_capacity(rs.len() + 1);
    let mut found = false;
    for r in rs {
        let Some(n) = normalize_reaction_content(&r.content) else {
            continue;
        };
        if r.count <= 0 {
            continue;
        }
        let mut r = PrReaction {
            content: n.to_owned(),
            ..r.clone()
        };
        if n != content {
            out.push(r);
            continue;
        }
        found = true;
        if r.mine {
            r.count -= 1;
            r.mine = false;
        } else {
            r.count += 1;
            r.mine = true;
        }
        if r.count > 0 {
            out.push(r);
        }
    }
    if !found {
        out.push(PrReaction {
            content: content.to_owned(),
            count: 1,
            mine: true,
        });
    }
    sort_reactions(&out)
}

/// Orders a rollup in GitHub picker order and drops empty groups. Unknown
/// contents sort with the first rank, keeping their relative order.
pub fn sort_reactions(rs: &[PrReaction]) -> Vec<PrReaction> {
    let mut kept: Vec<PrReaction> = rs
        .iter()
        .filter(|r| r.count > 0 && !r.content.is_empty())
        .cloned()
        .collect();
    let rank = |c: &str| {
        REACTIONS
            .iter()
            .position(|(rest, _)| *rest == c)
            .unwrap_or(0)
    };
    kept.sort_by_key(|r| rank(&r.content));
    kept
}

/// Reports whether the viewer already reacted with content.
pub fn reaction_mine(rs: &[PrReaction], content: &str) -> bool {
    let Some(content) = normalize_reaction_content(content) else {
        return false;
    };
    rs.iter()
        .find(|r| normalize_reaction_content(&r.content) == Some(content))
        .is_some_and(|r| r.mine)
}

/// Reports whether two reaction rollups match.
pub fn reactions_equal(a: &[PrReaction], b: &[PrReaction]) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(content: &str, count: i64, mine: bool) -> PrReaction {
        PrReaction {
            content: content.into(),
            count,
            mine,
        }
    }

    #[test]
    fn normalize_cases() {
        for (input, want) in [
            ("+1", Some("+1")),
            ("THUMBS_UP", Some("+1")),
            ("thumbs_up", Some("+1")),
            ("-1", Some("-1")),
            ("THUMBS_DOWN", Some("-1")),
            ("LAUGH", Some("laugh")),
            ("Heart", Some("heart")),
            (" eyes ", Some("eyes")),
            ("", None),
            ("wave", None),
        ] {
            assert_eq!(normalize_reaction_content(input), want, "{input:?}");
        }
    }

    #[test]
    fn graphql_cases() {
        assert_eq!(graphql_reaction_content("+1"), Some("THUMBS_UP"));
        assert_eq!(graphql_reaction_content("rocket"), Some("ROCKET"));
        assert_eq!(graphql_reaction_content("wave"), None);
    }

    #[test]
    fn toggle_cases() {
        let start = vec![r("heart", 1, true), r("+1", 2, false)];
        assert_eq!(
            toggle_reactions(&start, "rocket"),
            vec![r("+1", 2, false), r("heart", 1, true), r("rocket", 1, true)]
        );
        assert_eq!(toggle_reactions(&start, "heart"), vec![r("+1", 2, false)]);
        assert_eq!(
            toggle_reactions(&[r("+1", 2, false)], "+1"),
            vec![r("+1", 3, true)]
        );
        assert!(reactions_equal(&toggle_reactions(&start, "wave"), &start));
        assert_eq!(
            toggle_reactions(
                &[
                    r("THUMBS_UP", 1, false),
                    r("wave", 4, false),
                    r("eyes", 0, false)
                ],
                "HEART"
            ),
            vec![r("+1", 1, false), r("heart", 1, true)]
        );
    }

    #[test]
    fn sort_and_mine() {
        assert_eq!(
            sort_reactions(&[
                r("eyes", 1, false),
                r("", 2, false),
                r("wave", 1, false),
                r("+1", 0, false),
                r("-1", 3, false)
            ]),
            vec![r("wave", 1, false), r("-1", 3, false), r("eyes", 1, false)]
        );
        assert!(sort_reactions(&[]).is_empty());
        let rs = [r("THUMBS_UP", 1, true), r("heart", 1, false)];
        assert!(reaction_mine(&rs, "+1"));
        assert!(!reaction_mine(&rs, "heart"));
        assert!(!reaction_mine(&rs, "wave"));
        assert!(!reactions_equal(&rs, &rs[..1]));
    }
}
