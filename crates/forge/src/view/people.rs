//! Avatar stacks, reviewer summaries and @mention suggestions for PR views.

use std::collections::{HashMap, HashSet};

use recur_domain::{Pr, PrParticipant, PrReviewer, PrSource};

use crate::Forge;

/// Max faces in the PR-list stack, including a +N overflow chip.
pub const PR_FACE_CAP: usize = 6;

/// One face in a PR stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrPerson {
    pub login: String,
    pub name: String,
    pub avatar_url: String,
    pub avatar_css: String,
    pub code: String,
    /// Reviewer state when this person reviewed or was asked to.
    pub review_state: String,
}

impl From<&PrParticipant> for PrPerson {
    fn from(p: &PrParticipant) -> Self {
        Self {
            login: p.login.clone(),
            name: p.name.clone(),
            avatar_url: p.avatar_url.clone(),
            avatar_css: p.avatar_css.clone(),
            code: p.code.clone(),
            review_state: String::new(),
        }
    }
}

impl From<&PrReviewer> for PrPerson {
    fn from(r: &PrReviewer) -> Self {
        Self {
            login: r.login.clone(),
            name: r.name.clone(),
            avatar_url: r.avatar_url.clone(),
            avatar_css: r.avatar_css.clone(),
            code: r.code.clone(),
            review_state: r.state.clone(),
        }
    }
}

fn review_rank(state: &str) -> u8 {
    match state {
        "changes_requested" => 0,
        "pending" => 1,
        "approved" => 2,
        "commented" => 3,
        _ => 8,
    }
}

pub fn review_state_label(state: &str) -> &'static str {
    match state.trim() {
        "pending" => "Review requested",
        "approved" => "Approved",
        "changes_requested" => "Changes requested",
        "commented" => "Commented",
        _ => "",
    }
}

/// Reviewers that have a state.
pub fn pr_reviewers(pr: &Pr) -> Vec<PrReviewer> {
    pr.reviewers
        .iter()
        .filter(|r| !r.state.trim().is_empty())
        .cloned()
        .collect()
}

/// Dedupe key: login, else name, else code (case-insensitive).
pub fn person_key(p: &PrPerson) -> String {
    let login = p.login.trim().to_lowercase();
    if !login.is_empty() {
        return format!("login:{login}");
    }
    let name = p.name.trim().to_lowercase();
    if !name.is_empty() {
        return format!("name:{name}");
    }
    let code = p.code.trim().to_lowercase();
    if !code.is_empty() {
        return format!("code:{code}");
    }
    String::new()
}

/// Stored participants, or author then unique commenters.
pub fn pr_participants(pr: &Pr) -> Vec<PrPerson> {
    if !pr.participants.is_empty() {
        return pr.participants.iter().map(PrPerson::from).collect();
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |p: PrPerson| {
        let k = person_key(&p);
        if !k.is_empty() && seen.insert(k) {
            out.push(p);
        }
    };
    let author = pr.author_login.trim();
    if !author.is_empty() {
        add(PrPerson {
            login: author.to_owned(),
            ..Default::default()
        });
    }
    for c in pr.comments.iter().flatten() {
        if c.kind == "hidden" {
            continue;
        }
        add(PrPerson {
            login: c.author_login.clone(),
            name: c.author.clone(),
            avatar_url: c.avatar_url.clone(),
            avatar_css: c.avatar_css.clone(),
            code: c.code.clone(),
            review_state: String::new(),
        });
    }
    out
}

/// Reviewers first, then participants, each stamped with their review
/// state; ordered by state (changes requested, pending, approved,
/// commented, none).
pub fn pr_people_with_reviews(pr: &Pr) -> Vec<PrPerson> {
    let reviewers = pr_reviewers(pr);
    let by_key: HashMap<String, String> = reviewers
        .iter()
        .map(PrPerson::from)
        .filter_map(|p| {
            let k = person_key(&p);
            (!k.is_empty()).then(|| (k, p.review_state.trim().to_owned()))
        })
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut p in reviewers
        .iter()
        .map(PrPerson::from)
        .chain(pr_participants(pr))
    {
        let k = person_key(&p);
        if k.is_empty() || !seen.insert(k.clone()) {
            continue;
        }
        if let Some(st) = by_key.get(&k).filter(|s| !s.is_empty()) {
            p.review_state = st.clone();
        }
        out.push(p);
    }
    out.sort_by_key(|p| review_rank(&p.review_state));
    out
}

fn and_list(names: &[String]) -> String {
    if names.len() <= 2 {
        return names.join(" and ");
    }
    format!("{} +{}", names[..2].join(", "), names.len() - 2)
}

/// `Approved by @a · Changes requested by @b · Waiting on @c`.
pub fn reviewer_summary(reviewers: &[PrReviewer]) -> String {
    let mut approved = Vec::new();
    let mut changes = Vec::new();
    let mut pending = Vec::new();
    for r in reviewers {
        let bucket = match r.state.trim() {
            "approved" => &mut approved,
            "changes_requested" => &mut changes,
            "pending" => &mut pending,
            _ => continue,
        };
        let label = participant_label(&PrPerson::from(r));
        if !label.is_empty() {
            bucket.push(label);
        }
    }
    let mut parts = Vec::new();
    if !approved.is_empty() {
        parts.push(format!("Approved by {}", and_list(&approved)));
    }
    if !changes.is_empty() {
        parts.push(format!("Changes requested by {}", and_list(&changes)));
    }
    if !pending.is_empty() {
        parts.push(format!("Waiting on {}", and_list(&pending)));
    }
    parts.join(" · ")
}

/// `@login`, else the display name.
pub fn participant_label(p: &PrPerson) -> String {
    let login = mention_login(p);
    if !login.is_empty() {
        return format!("@{login}");
    }
    p.name.trim().to_owned()
}

/// Agent code, else the first two letters of login or name.
pub fn participant_initials(p: &PrPerson) -> String {
    let code = p.code.trim();
    if !code.is_empty() {
        return code.chars().take(2).collect::<String>().to_uppercase();
    }
    let s = if p.login.is_empty() {
        if p.name.is_empty() { "?" } else { &p.name }
    } else {
        &p.login
    };
    let s = s.trim();
    let s = s.strip_prefix('@').unwrap_or(s);
    s.chars().take(2).collect::<String>().to_uppercase()
}

/// The photo to show: the stored one, none for agents (CSS fill), else the
/// forge's photo for the login.
pub fn participant_avatar_url(p: &PrPerson, forge: &dyn Forge) -> String {
    let provided = p.avatar_url.trim();
    if !provided.is_empty() {
        return provided.to_owned();
    }
    if !p.avatar_css.trim().is_empty() {
        return String::new();
    }
    let login = mention_login(p);
    if login.is_empty() || login.eq_ignore_ascii_case("someone") {
        return String::new();
    }
    let login = match login.len().checked_sub(5) {
        Some(i) if login.is_char_boundary(i) && login[i..].eq_ignore_ascii_case("[bot]") => {
            &login[..i]
        }
        _ => login,
    };
    forge.avatar_url(login, "", 40)
}

/// Faces to draw, the overflow count, and the hidden people.
pub fn pr_face_slots(people: &[PrPerson]) -> (Vec<PrPerson>, usize, Vec<PrPerson>) {
    if people.len() <= PR_FACE_CAP {
        return (people.to_vec(), 0, Vec::new());
    }
    let shown = people[..PR_FACE_CAP - 1].to_vec();
    let rest = people[PR_FACE_CAP - 1..].to_vec();
    let extra = rest.len();
    (shown, extra, rest)
}

/// A person's login without a leading `@`.
pub fn mention_login(p: &PrPerson) -> &str {
    let l = p.login.trim();
    l.strip_prefix('@').unwrap_or(l)
}

/// People on a forge PR who can be @mentioned (Local PRs have none).
pub fn mention_people(pr: &Pr) -> Vec<PrPerson> {
    if pr.source == PrSource::Local {
        return Vec::new();
    }
    pr_people_with_reviews(pr)
        .into_iter()
        .filter(|p| !mention_login(p).is_empty())
        .collect()
}

/// Suggestions for an `@query`: people with a login, ranked on
/// `"<login> <name>"`. `rank(labels, query, limit)` returns label indexes
/// best first (the UI's fuzzy ranker, `recur_code::fuzzy`).
pub fn suggest_user_mentions(
    people: &[PrPerson],
    query: &str,
    limit: usize,
    rank: impl FnOnce(&[String], &str, usize) -> Vec<usize>,
) -> Vec<PrPerson> {
    let with_login: Vec<&PrPerson> = people
        .iter()
        .filter(|p| !mention_login(p).is_empty())
        .collect();
    let q = query.trim();
    if q.is_empty() {
        return with_login.into_iter().take(limit).cloned().collect();
    }
    let labels: Vec<String> = with_login
        .iter()
        .map(|p| format!("{} {}", mention_login(p), p.name).trim().to_owned())
        .collect();
    rank(&labels, q, limit)
        .into_iter()
        .filter_map(|i| with_login.get(i).map(|p| (*p).clone()))
        .collect()
}

/// Local hits first, then remote ones, deduped by login, capped.
pub fn merge_mention_hits(local: &[PrPerson], remote: &[PrPerson], limit: usize) -> Vec<PrPerson> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for p in local.iter().chain(remote) {
        let login = mention_login(p).to_lowercase();
        if login.is_empty() || !seen.insert(login) {
            continue;
        }
        out.push(p.clone());
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use recur_domain::PrComment;

    use super::*;
    use crate::github::GitHub;

    fn person(login: &str, name: &str) -> PrPerson {
        PrPerson {
            login: login.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn labels_initials_avatars() {
        assert_eq!(
            participant_label(&person("avery", "Avery Rourke")),
            "@avery"
        );
        assert_eq!(participant_label(&person("", "Coder")), "Coder");
        assert_eq!(participant_label(&person("@sam", "")), "@sam");
        let coder = PrPerson {
            code: "CO".into(),
            ..person("coder", "")
        };
        assert_eq!(participant_initials(&coder), "CO");
        assert_eq!(participant_initials(&person("avery", "")), "AV");
        assert_eq!(participant_initials(&person("", "PR Responder")), "PR");
        let gh = GitHub::new();
        let with_url = PrPerson {
            avatar_url: "https://ex/a.png".into(),
            ..Default::default()
        };
        assert_eq!(participant_avatar_url(&with_url, &gh), "https://ex/a.png");
        assert_eq!(
            participant_avatar_url(&person("avery", ""), &gh),
            "https://github.com/avery.png?size=40"
        );
        assert_eq!(
            participant_avatar_url(&person("dependabot[bot]", ""), &gh),
            "https://github.com/dependabot.png?size=40"
        );
        let css = PrPerson {
            avatar_css: "background:red".into(),
            ..person("loopbot", "")
        };
        assert_eq!(participant_avatar_url(&css, &gh), "");
        assert_eq!(participant_avatar_url(&person("", "Coder"), &gh), "");
    }

    #[test]
    fn face_slots() {
        let six: Vec<PrPerson> = (0..6).map(|i| person(&format!("u{i}"), "")).collect();
        assert_eq!(pr_face_slots(&six), (six.clone(), 0, Vec::new()));
        let eight: Vec<PrPerson> = (0..8).map(|i| person(&format!("u{i}"), "")).collect();
        let (shown, extra, rest) = pr_face_slots(&eight);
        assert_eq!((shown.len(), extra), (5, 3));
        assert_eq!(
            rest.iter().map(|p| p.login.as_str()).collect::<Vec<_>>(),
            ["u5", "u6", "u7"]
        );
    }

    #[test]
    fn participants_stored_or_fallback() {
        let stored = Pr {
            participants: vec![PrParticipant {
                login: "sam".into(),
                ..Default::default()
            }],
            author_login: "other".into(),
            ..Default::default()
        };
        assert_eq!(pr_participants(&stored), [person("sam", "")]);
        let c = |kind: &str, login: &str, author: &str, code: &str| PrComment {
            kind: kind.into(),
            author_login: login.into(),
            author: author.into(),
            code: code.into(),
            ..Default::default()
        };
        let fallback = Pr {
            author_login: "loopbot".into(),
            comments: Some(vec![
                c("description", "loopbot", "loopbot", ""),
                c("", "avery", "Avery Rourke", "AR"),
                c("hidden", "ghost", "", ""),
                c("", "", "Coder", "CO"),
            ]),
            ..Default::default()
        };
        let got = pr_participants(&fallback);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], person("loopbot", ""));
        assert_eq!(
            (got[1].login.as_str(), got[1].code.as_str()),
            ("avery", "AR")
        );
        assert_eq!(
            (got[2].name.as_str(), got[2].code.as_str()),
            ("Coder", "CO")
        );
    }

    fn reviewer(login: &str, state: &str) -> PrReviewer {
        PrReviewer {
            login: login.into(),
            state: state.into(),
            ..Default::default()
        }
    }

    #[test]
    fn people_with_reviews_and_summary() {
        assert_eq!(review_state_label("approved"), "Approved");
        assert_eq!(review_state_label("pending"), "Review requested");
        assert_eq!(review_state_label("changes_requested"), "Changes requested");
        assert_eq!(review_state_label("commented"), "Commented");
        assert_eq!(review_state_label(""), "");
        let pr = Pr {
            participants: vec![
                PrParticipant {
                    login: "loopbot".into(),
                    ..Default::default()
                },
                PrParticipant {
                    login: "avery".into(),
                    ..Default::default()
                },
                PrParticipant {
                    name: "Coder".into(),
                    code: "CO".into(),
                    ..Default::default()
                },
            ],
            reviewers: vec![reviewer("sam", "approved"), reviewer("avery", "pending")],
            ..Default::default()
        };
        let got: Vec<String> = pr_people_with_reviews(&pr)
            .iter()
            .map(|p| {
                format!(
                    "{}:{}",
                    if p.login.is_empty() {
                        &p.name
                    } else {
                        &p.login
                    },
                    p.review_state
                )
            })
            .collect();
        assert_eq!(got, ["avery:pending", "sam:approved", "loopbot:", "Coder:"]);
        assert_eq!(
            reviewer_summary(&[
                reviewer("sam", "approved"),
                reviewer("avery", "pending"),
                reviewer("lee", "changes_requested")
            ]),
            "Approved by @sam · Changes requested by @lee · Waiting on @avery"
        );
        assert_eq!(
            reviewer_summary(&[
                reviewer("a", "approved"),
                reviewer("b", "approved"),
                reviewer("c", "approved")
            ]),
            "Approved by @a, @b +1"
        );
    }

    #[test]
    fn mention_suggestions() {
        let people = [
            person("avery", "Avery Rourke"),
            person("loopbot", "loopbot"),
            person("sam", "sam"),
            PrPerson {
                code: "CO".into(),
                ..person("", "Coder")
            },
        ];
        let never = |_: &[String], _: &str, _: usize| -> Vec<usize> { unreachable!() };
        let got: Vec<String> = suggest_user_mentions(&people, "", 8, never)
            .into_iter()
            .map(|p| p.login)
            .collect();
        assert_eq!(got, ["avery", "loopbot", "sam"]);
        let many: Vec<PrPerson> = (0..12).map(|i| person(&format!("u{i}"), "")).collect();
        assert_eq!(suggest_user_mentions(&many, " ", 8, never).len(), 8);
        let contains = |labels: &[String], q: &str, limit: usize| -> Vec<usize> {
            let q = q.to_lowercase();
            (0..labels.len())
                .filter(|&i| labels[i].to_lowercase().contains(&q))
                .take(limit)
                .collect()
        };
        let got: Vec<String> = suggest_user_mentions(&people, "rourke", 8, contains)
            .into_iter()
            .map(|p| p.login)
            .collect();
        assert_eq!(got, ["avery"]);
        assert!(suggest_user_mentions(&people, "Coder", 8, contains).is_empty());

        let local = [person("avery", "Avery"), person("loopbot", "")];
        let remote = [
            person("Avery", "Avery Rourke"),
            person("octocat", "The Octocat"),
            person("hubot", ""),
        ];
        let logins = |v: Vec<PrPerson>| v.into_iter().map(|p| p.login).collect::<Vec<_>>();
        assert_eq!(
            logins(merge_mention_hits(&local, &remote, 3)),
            ["avery", "loopbot", "octocat"]
        );
        assert_eq!(
            logins(merge_mention_hits(&local, &[], 8)),
            ["avery", "loopbot"]
        );
        assert_eq!(
            logins(merge_mention_hits(&[], &remote, 8)),
            ["Avery", "octocat", "hubot"]
        );
    }

    #[test]
    fn mention_people_only_on_forge_prs() {
        let pr = Pr {
            source: PrSource::GitHub,
            participants: vec![
                PrParticipant {
                    login: "avery".into(),
                    ..Default::default()
                },
                PrParticipant {
                    name: "Coder".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(mention_people(&pr).len(), 1);
        let local = Pr {
            source: PrSource::Local,
            ..pr
        };
        assert!(mention_people(&local).is_empty());
    }
}
