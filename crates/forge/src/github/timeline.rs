//! The REST issue timeline as conversation action rows. GraphQL
//! `timelineItems` omits Copilot work events, so the snapshot keeps REST.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use recur_domain::PrComment;
use serde::Deserialize;

use super::display_name;
use super::thread::{Actor, action_comment};
use crate::CommitMeta;
use crate::json::decode;
use crate::model::lookup_by_sha;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Event {
    id: i64,
    html_url: String,
    event: String,
    created_at: String,
    submitted_at: String,
    sha: String,
    commit_id: String,
    message: String,
    actor: Option<Actor>,
    review_requester: Option<Actor>,
    requested_reviewer: Option<Actor>,
    requested_team: Option<Named>,
    author: Option<GitAuthor>,
    label: Option<Named>,
    rename: Option<Rename>,
    assignee: Option<Actor>,
    performed_via_github_app: Option<App>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct GitAuthor {
    name: String,
    date: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Rename {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct App {
    slug: String,
}

fn event_url(ev: &Event, pr_url: &str, commit_url: &str) -> String {
    let u = ev.html_url.trim();
    if !u.is_empty() {
        return u.to_owned();
    }
    let u = commit_url.trim();
    if !u.is_empty() {
        return u.to_owned();
    }
    let base = pr_url.trim().trim_end_matches('/');
    if base.is_empty() || ev.id <= 0 {
        return String::new();
    }
    format!("{base}#event-{}", ev.id)
}

fn with_url(mut c: PrComment, ev: &Event, pr_url: &str, commit_url: &str) -> PrComment {
    if c.url.trim().is_empty() {
        c.url = event_url(ev, pr_url, commit_url);
    }
    c
}

fn actor_or(primary: Option<&Actor>, fallback: Option<&Actor>) -> Actor {
    if let Some(p) = primary
        && !p.login.trim().is_empty()
    {
        return p.clone();
    }
    fallback.cloned().unwrap_or_else(|| Actor::named("someone"))
}

/// Go `parseGitHubTimeline`. `commit_logins` maps commit oids to linked
/// GitHub logins; `head` and `base` name the branches in ref events.
pub(crate) fn parse_timeline(
    raw: &str,
    now: DateTime<Utc>,
    commit_logins: &HashMap<String, String>,
    meta: &HashMap<String, CommitMeta>,
    pr_url: &str,
    head: &str,
    base: &str,
) -> Vec<PrComment> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "[]" {
        return Vec::new();
    }
    let Ok(rows) = decode::<Vec<Event>>(raw) else {
        return Vec::new();
    };
    let has_merged = rows
        .iter()
        .any(|ev| ev.event.trim().eq_ignore_ascii_case("merged"));
    let mut out = Vec::new();
    for (i, ev) in rows.iter().enumerate() {
        let kind = ev.event.trim().to_lowercase();
        let mut when = ev.created_at.clone();
        if when.is_empty() {
            when = ev.submitted_at.clone();
        }
        if let Some(a) = &ev.author
            && !a.date.is_empty()
            && when.is_empty()
        {
            when = a.date.clone();
        }
        let id = format!("gh-event-{kind}-{i}-{when}");
        let actor = || actor_or(ev.actor.as_ref(), None);
        let act = |body: &str| {
            with_url(
                action_comment(&id, &actor(), body, &when, now),
                ev,
                pr_url,
                "",
            )
        };
        match kind.as_str() {
            "committed" => {
                let mut who = match &ev.author {
                    Some(a) if !a.name.is_empty() => Actor::named(&a.name),
                    _ => Actor::named("someone"),
                };
                let linked = lookup_by_sha(commit_logins, &ev.sha).filter(|l| !l.is_empty());
                if let Some(login) = linked {
                    who = Actor::named(login);
                }
                let mut msg = ev.message.trim().to_owned();
                if let Some(nl) = msg.find('\n') {
                    msg = msg[..nl].trim().to_owned();
                }
                if msg.is_empty() {
                    msg = ev.sha.clone();
                }
                let mut c = action_comment(&id, &who, &format!("committed {msg}"), &when, now);
                if linked.is_none() {
                    c.avatar_url.clear();
                }
                c.commit_sha = ev.sha.trim().to_owned();
                let mut commit_url = String::new();
                if let Some(cm) = lookup_by_sha(meta, &ev.sha) {
                    c.verified = cm.verified;
                    c.check_status = cm.check_status.clone();
                    c.url = cm.url.clone();
                    commit_url = cm.url.clone();
                }
                out.push(with_url(c, ev, pr_url, &commit_url));
            }
            "review_requested" | "review_request_removed" => {
                let who = actor_or(ev.review_requester.as_ref(), ev.actor.as_ref());
                let mut target = "a reviewer".to_owned();
                if let Some(r) = ev
                    .requested_reviewer
                    .as_ref()
                    .filter(|r| !r.login.is_empty())
                {
                    target = display_name(&r.login);
                } else if kind == "review_requested"
                    && let Some(t) = ev.requested_team.as_ref().filter(|t| !t.name.is_empty())
                {
                    target = t.name.clone();
                }
                let body = if kind == "review_requested" {
                    format!("requested a review from {target}")
                } else {
                    format!("removed the review request for {target}")
                };
                out.push(with_url(
                    action_comment(&id, &who, &body, &when, now),
                    ev,
                    pr_url,
                    "",
                ));
            }
            "ready_for_review" => out.push(act("marked this pull request as ready for review")),
            "convert_to_draft" => out.push(act("converted this pull request to draft")),
            "closed" => {
                if has_merged || !ev.commit_id.trim().is_empty() {
                    continue;
                }
                out.push(glyph_action(&id, ev, "closed this", &when, now, pr_url));
            }
            "reopened" => out.push(act("reopened this")),
            "merged" => {
                let mut c = glyph_action(&id, ev, &merge_event_body(ev, base), &when, now, pr_url);
                c.commit_sha = event_sha(ev);
                out.push(c);
            }
            "head_ref_deleted" => {
                out.push(glyph_action(
                    &id,
                    ev,
                    &head_ref_event_body("deleted", head),
                    &when,
                    now,
                    pr_url,
                ));
            }
            "head_ref_restored" => {
                out.push(glyph_action(
                    &id,
                    ev,
                    &head_ref_event_body("restored", head),
                    &when,
                    now,
                    pr_url,
                ));
            }
            "head_ref_force_pushed" => out.push(act("force-pushed the branch")),
            "assigned" | "unassigned" => {
                let target = ev
                    .assignee
                    .as_ref()
                    .filter(|a| !a.login.is_empty())
                    .map_or_else(|| "someone".to_owned(), |a| display_name(&a.login));
                out.push(act(&format!("{kind} {target}")));
            }
            "labeled" | "unlabeled" => {
                let label = ev
                    .label
                    .as_ref()
                    .filter(|l| !l.name.trim().is_empty())
                    .map_or_else(|| "a label".to_owned(), |l| l.name.clone());
                let verb = if kind == "labeled" {
                    "added"
                } else {
                    "removed"
                };
                out.push(act(&format!("{verb} the {label} label")));
            }
            "renamed" => {
                let body = match &ev.rename {
                    Some(r) if !r.from.is_empty() && !r.to.is_empty() => {
                        format!("changed the title from {} to {}", r.from, r.to)
                    }
                    _ => "changed the title".to_owned(),
                };
                out.push(act(&body));
            }
            "copilot_work_started" | "copilot_work_finished" | "copilot_work_finished_failure" => {
                if let Some(c) = copilot_work_action(&id, &kind, ev, &when, now) {
                    out.push(with_url(c, ev, pr_url, ""));
                }
            }
            _ => {}
        }
    }
    out
}

/// An event GitHub draws with a branch/state glyph instead of an avatar.
fn glyph_action(
    id: &str,
    ev: &Event,
    body: &str,
    when: &str,
    now: DateTime<Utc>,
    pr_url: &str,
) -> PrComment {
    let mut c = action_comment(id, &actor_or(ev.actor.as_ref(), None), body, when, now);
    c.avatar_url.clear();
    with_url(c, ev, pr_url, "")
}

fn event_sha(ev: &Event) -> String {
    let s = ev.sha.trim();
    if s.is_empty() {
        ev.commit_id.trim().to_owned()
    } else {
        s.to_owned()
    }
}

/// The first seven characters of a commit sha.
pub(crate) fn short_sha(sha: &str) -> String {
    sha.trim().chars().take(7).collect()
}

fn merge_event_body(ev: &Event, base: &str) -> String {
    let sha = short_sha(&event_sha(ev));
    let base = base.trim();
    match (sha.is_empty(), base.is_empty()) {
        (false, false) => format!("merged commit {sha} into {base}"),
        (false, true) => format!("merged commit {sha}"),
        (true, false) => format!("merged this into {base}"),
        (true, true) => "merged this pull request".to_owned(),
    }
}

fn head_ref_event_body(verb: &str, head: &str) -> String {
    let head = head.trim();
    if head.is_empty() {
        format!("{verb} the head branch")
    } else {
        format!("{verb} the {head} branch")
    }
}

fn copilot_work_action(
    id: &str,
    kind: &str,
    ev: &Event,
    when: &str,
    now: DateTime<Utc>,
) -> Option<PrComment> {
    let behalf = ev
        .actor
        .as_ref()
        .filter(|a| !a.login.trim().is_empty())
        .map_or_else(|| "someone".to_owned(), |a| display_name(&a.login));
    let review = ev
        .performed_via_github_app
        .as_ref()
        .is_some_and(|a| a.slug.trim().to_lowercase() == "copilot-pull-request-reviewer");
    let verb = match kind {
        "copilot_work_started" if review => format!("started reviewing on behalf of {behalf}"),
        "copilot_work_started" => format!("started work on behalf of {behalf}"),
        "copilot_work_finished" if review => format!("finished reviewing on behalf of {behalf}"),
        "copilot_work_finished" => format!("finished work on behalf of {behalf}"),
        "copilot_work_finished_failure" => {
            format!("stopped work on behalf of {behalf} due to an error")
        }
        _ => return None,
    };
    let copilot = Actor {
        login: "copilot-pull-request-reviewer".into(),
        kind: "Bot".into(),
        ..Default::default()
    };
    let mut c = action_comment(id, &copilot, &verb, when, now);
    // GitHub uses the branch glyph here, not Copilot's avatar.
    c.avatar_url.clear();
    Some(c)
}
