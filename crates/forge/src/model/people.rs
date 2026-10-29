//! PR participants (avatar stacks) and reviewers (review state per person).
//!
//! `avatar` resolves a login plus a provided avatar URL to the URL to show;
//! forges pass [`crate::Forge::avatar_url`] so a login-only person still
//! gets a photo.

use recur_domain::{
    Commit, Pr, PrComment, PrParticipant, PrReviewer, REVIEW_STATE_APPROVED,
    REVIEW_STATE_CHANGES_REQUESTED, REVIEW_STATE_COMMENTED, REVIEW_STATE_PENDING,
};

use super::text::comment_avatar_css;
use crate::PrSnapshot;

/// Resolves `(login, provided avatar)` to the avatar URL to store.
pub type AvatarFn<'a> = &'a dyn Fn(&str, &str) -> String;

/// Dedupe key: login, else display name, else agent code (case-insensitive).
pub fn participant_key(p: &PrParticipant) -> String {
    key_of(&p.login, &p.name, &p.code)
}

fn key_of(login: &str, name: &str, code: &str) -> String {
    let login = login.trim().to_lowercase();
    if !login.is_empty() {
        return format!("login:{login}");
    }
    let name = name.trim().to_lowercase();
    if !name.is_empty() {
        return format!("name:{name}");
    }
    let code = code.trim().to_lowercase();
    if !code.is_empty() {
        return format!("code:{code}");
    }
    String::new()
}

/// Appends `p` (trimmed) unless it has no key or is already present.
pub fn append_participant(out: &mut Vec<PrParticipant>, p: PrParticipant) {
    let p = PrParticipant {
        login: p.login.trim().to_owned(),
        name: p.name.trim().to_owned(),
        avatar_url: p.avatar_url.trim().to_owned(),
        avatar_css: p.avatar_css.trim().to_owned(),
        code: p.code.trim().to_owned(),
    };
    let key = participant_key(&p);
    if key.is_empty() || out.iter().any(|have| participant_key(have) == key) {
        return;
    }
    out.push(p);
}

/// `base` in order, then each new person from `extra`.
pub fn merge_participants(base: &[PrParticipant], extra: &[PrParticipant]) -> Vec<PrParticipant> {
    let mut out = base.to_vec();
    for p in extra {
        append_participant(&mut out, p.clone());
    }
    out
}

/// The person who wrote a conversation row.
pub fn comment_participant(c: &PrComment, avatar: AvatarFn) -> PrParticipant {
    let login = c.author_login.trim();
    PrParticipant {
        login: login.to_owned(),
        name: c.author.trim().to_owned(),
        avatar_url: avatar(login, c.avatar_url.trim()),
        avatar_css: c.avatar_css.clone(),
        code: c.code.trim().to_owned(),
    }
}

/// The author of a commit: an agent (code + role) or a git/forge user.
pub fn commit_participant(c: &Commit, avatar: AvatarFn) -> PrParticipant {
    let code = c.agent_code.trim();
    let author = c.author.trim();
    let mut name = c.author_role.trim();
    if name.is_empty() {
        name = author;
    }
    let login = if code.is_empty() && !author.is_empty() && !author.contains(' ') {
        author
    } else {
        ""
    };
    PrParticipant {
        login: login.to_owned(),
        name: name.to_owned(),
        avatar_url: avatar(login, c.avatar_url.trim()),
        avatar_css: if code.is_empty() {
            String::new()
        } else {
            comment_avatar_css(code).to_owned()
        },
        code: code.to_owned(),
    }
}

/// Author first, then stored participants, commenters and commit authors.
pub fn participants_from_pr(pr: &Pr, avatar: AvatarFn) -> Vec<PrParticipant> {
    let mut out = Vec::new();
    let login = pr.author_login.trim();
    if !login.is_empty() {
        append_participant(
            &mut out,
            PrParticipant {
                login: login.to_owned(),
                avatar_url: avatar(login, ""),
                ..Default::default()
            },
        );
    }
    out = merge_participants(&out, &pr.participants);
    for c in pr.comments.iter().flatten() {
        if c.kind != "hidden" {
            append_participant(&mut out, comment_participant(c, avatar));
        }
    }
    for c in pr.commits.iter().flatten() {
        append_participant(&mut out, commit_participant(c, avatar));
    }
    out
}

/// Author, assignees, requested reviewers, then commenters of a snapshot.
pub fn snapshot_participants(
    snap: &PrSnapshot,
    comments: &[PrComment],
    avatar: AvatarFn,
) -> Vec<PrParticipant> {
    let mut out = Vec::new();
    let login = snap.author_login.trim();
    if !login.is_empty() {
        append_participant(
            &mut out,
            PrParticipant {
                login: login.to_owned(),
                avatar_url: snap.author_avatar_url.clone(),
                ..Default::default()
            },
        );
    }
    for p in snap.assignees.iter().chain(&snap.review_requests) {
        append_participant(&mut out, p.clone());
    }
    for c in comments {
        if c.kind != "hidden" {
            append_participant(&mut out, comment_participant(c, avatar));
        }
    }
    out
}

/// Dedupe key of a reviewer (same rules as [`participant_key`]).
pub fn reviewer_key(r: &PrReviewer) -> String {
    key_of(&r.login, &r.name, &r.code)
}

fn reviewer_from(p: PrParticipant, state: &str) -> PrReviewer {
    PrReviewer {
        login: p.login,
        name: p.name,
        avatar_url: p.avatar_url,
        avatar_css: p.avatar_css,
        code: p.code,
        state: state.to_owned(),
    }
}

/// A reviewer for a forge user (already resolved, see [`crate::PrListRow`]).
pub fn actor_reviewer(actor: &PrParticipant, state: &str) -> PrReviewer {
    reviewer_from(actor.clone(), state)
}

/// Adds `r`, or updates the existing entry's state and fills its avatar.
pub fn upsert_reviewer(out: &mut Vec<PrReviewer>, mut r: PrReviewer) {
    r.state = r.state.trim().to_owned();
    let key = reviewer_key(&r);
    if key.is_empty() || r.state.is_empty() {
        return;
    }
    if let Some(have) = out.iter_mut().find(|have| reviewer_key(have) == key) {
        if !r.avatar_url.is_empty() {
            have.avatar_url = r.avatar_url;
        }
        if !r.avatar_css.is_empty() {
            have.avatar_css = r.avatar_css;
        }
        if !r.name.is_empty() && have.name.is_empty() {
            have.name = r.name;
        }
        have.state = r.state;
        return;
    }
    out.push(r);
}

/// Marks a requested reviewer pending, unless they already approved or
/// requested changes.
pub fn request_reviewer(out: &mut Vec<PrReviewer>, actor: &PrParticipant) {
    let key = participant_key(actor);
    if key.is_empty() {
        return;
    }
    if let Some(have) = out.iter().find(|have| reviewer_key(have) == key)
        && (have.state == REVIEW_STATE_APPROVED || have.state == REVIEW_STATE_CHANGES_REQUESTED)
    {
        return;
    }
    upsert_reviewer(out, actor_reviewer(actor, REVIEW_STATE_PENDING));
}

/// Sort rank: changes requested, pending, approved, commented.
pub fn review_state_rank(state: &str) -> u8 {
    match state {
        REVIEW_STATE_CHANGES_REQUESTED => 0,
        REVIEW_STATE_PENDING => 1,
        REVIEW_STATE_APPROVED => 2,
        REVIEW_STATE_COMMENTED => 3,
        _ => 9,
    }
}

/// A forge review state (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`) as a
/// reviewer state; empty for others (dismissed, pending).
pub fn normalize_review_state(state: &str) -> &'static str {
    match state.trim().to_uppercase().as_str() {
        "APPROVED" => REVIEW_STATE_APPROVED,
        "CHANGES_REQUESTED" => REVIEW_STATE_CHANGES_REQUESTED,
        "COMMENTED" => REVIEW_STATE_COMMENTED,
        _ => "",
    }
}

/// The reviewer state a conversation verb implies.
pub fn review_state_from_verb(verb: &str) -> &'static str {
    match verb.trim().to_lowercase().as_str() {
        "approved" => REVIEW_STATE_APPROVED,
        "requested changes" | "changes requested" => REVIEW_STATE_CHANGES_REQUESTED,
        "commented" | "reviewed" => REVIEW_STATE_COMMENTED,
        _ => "",
    }
}

/// The review state a conversation row records, or empty when it is not a
/// review (plain comments only count when they approve or request changes).
pub fn comment_review_state(c: &PrComment) -> &'static str {
    if c.kind == "hidden" || c.kind == "description" {
        return "";
    }
    let st = review_state_from_verb(&c.verb);
    if st == REVIEW_STATE_APPROVED || st == REVIEW_STATE_CHANGES_REQUESTED {
        return st;
    }
    if c.is_review_card() {
        return if st.is_empty() {
            REVIEW_STATE_COMMENTED
        } else {
            st
        };
    }
    ""
}

/// Stable sort by state rank, then lower-cased key.
pub fn sort_reviewers(mut out: Vec<PrReviewer>) -> Vec<PrReviewer> {
    out.sort_by(|a, b| {
        review_state_rank(&a.state)
            .cmp(&review_state_rank(&b.state))
            .then_with(|| {
                reviewer_key(a)
                    .to_lowercase()
                    .cmp(&reviewer_key(b).to_lowercase())
            })
    });
    out
}

/// Reviewers from conversation rows; the latest review per person wins.
pub fn comments_reviewers(comments: &[PrComment], avatar: AvatarFn) -> Vec<PrReviewer> {
    let mut out = Vec::new();
    for c in comments {
        let st = comment_review_state(c);
        if !st.is_empty() {
            upsert_reviewer(&mut out, reviewer_from(comment_participant(c, avatar), st));
        }
    }
    sort_reviewers(out)
}

/// Conversation reviewers plus outstanding review requests.
pub fn snapshot_reviewers(
    snap: &PrSnapshot,
    comments: &[PrComment],
    avatar: AvatarFn,
) -> Vec<PrReviewer> {
    let mut out = comments_reviewers(comments, avatar);
    for a in &snap.review_requests {
        request_reviewer(&mut out, a);
    }
    sort_reviewers(out)
}

/// Keeps a provided avatar and invents none.
pub fn provided_avatar(_login: &str, provided: &str) -> String {
    provided.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::avatar_url;

    fn gh(login: &str, provided: &str) -> String {
        avatar_url(login, provided, 56)
    }

    fn person(login: &str) -> PrParticipant {
        PrParticipant {
            login: login.into(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_participants_keeps_existing_order() {
        let base = [
            person("loopbot"),
            PrParticipant {
                name: "Coder".into(),
                code: "CO".into(),
                ..Default::default()
            },
        ];
        let got = merge_participants(&base, &[person("avery"), person("loopbot")]);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].login, "loopbot");
        assert_eq!(got[1].code, "CO");
        assert_eq!(got[2].login, "avery");
    }

    #[test]
    fn participants_from_pr_author_first() {
        let pr = Pr {
            author_login: "loopbot".into(),
            comments: Some(vec![
                PrComment {
                    kind: "description".into(),
                    author: "loopbot".into(),
                    author_login: "loopbot".into(),
                    code: "LO".into(),
                    ..Default::default()
                },
                PrComment {
                    author: "Avery Rourke".into(),
                    author_login: "avery".into(),
                    code: "AR".into(),
                    ..Default::default()
                },
                PrComment {
                    kind: "hidden".into(),
                    author: "ghost".into(),
                    author_login: "ghost".into(),
                    ..Default::default()
                },
                PrComment {
                    author: "Coder".into(),
                    code: "CO".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let got = participants_from_pr(&pr, &gh);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].login, "loopbot");
        assert_eq!(got[1].login, "avery");
        assert_eq!(got[2].code, "CO");
        assert_eq!(got[0].avatar_url, "https://github.com/loopbot.png?size=56");
    }

    #[test]
    fn commit_participant_agent_or_login() {
        let agent = commit_participant(
            &Commit {
                agent_code: "CO".into(),
                author_role: "Coder".into(),
                author: "josh".into(),
                ..Default::default()
            },
            &gh,
        );
        assert_eq!((agent.login.as_str(), agent.name.as_str()), ("", "Coder"));
        assert_eq!(agent.avatar_css, comment_avatar_css("CO"));
        let human = commit_participant(
            &Commit {
                author: "Josh Dechant".into(),
                ..Default::default()
            },
            &gh,
        );
        assert_eq!(
            (human.login.as_str(), human.name.as_str()),
            ("", "Josh Dechant")
        );
        let login = commit_participant(
            &Commit {
                author: "mycodecrafting".into(),
                ..Default::default()
            },
            &gh,
        );
        assert_eq!(login.login, "mycodecrafting");
        assert_eq!(
            login.avatar_url,
            "https://github.com/mycodecrafting.png?size=56"
        );
    }

    #[test]
    fn comments_reviewers_latest_wins() {
        let c = |id: &str, login: &str, author: &str, verb: &str| PrComment {
            id: id.into(),
            author_login: login.into(),
            author: author.into(),
            verb: verb.into(),
            ..Default::default()
        };
        let got = comments_reviewers(
            &[
                c("gh-review-1", "lee", "", "requested changes"),
                c("gh-review-2", "lee", "", "approved"),
                c("c-issue", "sam", "", "commented"),
                c("c-local", "", "You", "approved"),
            ],
            &gh,
        );
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(
            (got[0].login.as_str(), got[0].state.as_str()),
            ("lee", REVIEW_STATE_APPROVED)
        );
        assert_eq!(
            (got[1].name.as_str(), got[1].state.as_str()),
            ("You", REVIEW_STATE_APPROVED)
        );
    }

    #[test]
    fn snapshot_reviewers_pending_when_only_commented() {
        let snap = PrSnapshot {
            review_requests: vec![person("avery")],
            ..Default::default()
        };
        let got = snapshot_reviewers(
            &snap,
            &[PrComment {
                id: "gh-review-9".into(),
                author_login: "avery".into(),
                verb: "commented".into(),
                ..Default::default()
            }],
            &gh,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].login.as_str(), got[0].state.as_str()),
            ("avery", REVIEW_STATE_PENDING)
        );
    }

    #[test]
    fn request_keeps_approval() {
        let mut out = vec![actor_reviewer(&person("sam"), REVIEW_STATE_APPROVED)];
        request_reviewer(&mut out, &person("sam"));
        assert_eq!(out[0].state, REVIEW_STATE_APPROVED);
        let mut out = vec![actor_reviewer(&person("pat"), REVIEW_STATE_COMMENTED)];
        request_reviewer(&mut out, &person("pat"));
        assert_eq!(out[0].state, REVIEW_STATE_PENDING);
    }
}
