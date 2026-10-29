//! GitHub conversation parsing: PR view JSON, review line notes, node ids and
//! permalinks, avatars, review nesting, review threads, and comment merging.

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use chrono::{DateTime, Utc};
use recur_domain::{Commit, PrComment, PrParticipant};
use serde::Deserialize;

use super::{avatar_url, display_name};
use crate::json::decode;
use crate::model::{comment_avatar_css, compact_ago, format_time, format_when, parse_time};
use crate::{CommitAuthor, CommitMeta, PrSnapshot};

/// A GitHub user as `gh pr view` / GraphQL / REST return it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Actor {
    pub login: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: String,
    #[serde(rename = "avatar_url")]
    pub avatar_url_snake: String,
}

impl Actor {
    pub fn named(login: &str) -> Self {
        Self {
            login: login.to_owned(),
            ..Default::default()
        }
    }

    pub fn avatar(&self) -> &str {
        let u = self.avatar_url.trim();
        if u.is_empty() {
            self.avatar_url_snake.trim()
        } else {
            u
        }
    }
}

/// A `{login, avatarUrl}` row from `gh pr list` and GraphQL connections.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ListedActor {
    pub login: String,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: String,
}

impl ListedActor {
    /// Go `actorParticipant`.
    pub fn participant(&self) -> PrParticipant {
        let login = self.login.trim();
        PrParticipant {
            login: login.to_owned(),
            avatar_url: avatar_url(login, self.avatar_url.trim(), 56),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ViewJson {
    pub id: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub review_decision: String,
    pub mergeable: String,
    pub merge_state_status: String,
    pub is_draft: bool,
    pub head_ref_oid: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    pub author: Actor,
    pub comments: Vec<ViewComment>,
    pub reviews: Vec<ViewReview>,
    pub commits: Vec<ViewCommit>,
    pub review_requests: Vec<ListedActor>,
    pub assignees: Vec<ListedActor>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ViewCommit {
    pub oid: String,
    pub message_headline: String,
    pub message_body: String,
    pub committed_date: String,
    pub authors: Vec<ViewCommitAuthor>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ViewCommitAuthor {
    pub login: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ViewComment {
    pub id: String,
    pub url: String,
    pub author: Actor,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ViewReview {
    pub id: String,
    pub url: String,
    pub author: Actor,
    pub body: String,
    pub state: String,
    pub submitted_at: String,
}

/// One REST pull-request review comment (line note).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct InlineComment {
    pub id: i64,
    pub html_url: String,
    pub body: String,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: i64,
    pub start_line: Option<i64>,
    pub original_start_line: i64,
    pub side: String,
    pub start_side: String,
    pub diff_hunk: String,
    pub created_at: String,
    pub pull_request_review_id: i64,
    pub in_reply_to_id: i64,
    pub user: Actor,
}

impl InlineComment {
    fn comment_line(&self) -> i64 {
        match self.line {
            Some(l) if l > 0 => l,
            _ => self.original_line,
        }
    }

    fn outdated(&self) -> bool {
        self.line.is_none()
    }

    fn comment_start_line(&self) -> i64 {
        let mut start = self.start_line.unwrap_or(0);
        if start <= 0 {
            start = self.original_start_line;
        }
        let end = self.comment_line();
        if start <= 0 || start >= end { 0 } else { start }
    }
}

/// A snapshot under construction. `commit_logins` keeps the raw GitHub login
/// per commit for timeline rows; [`super::snapshot::finish`] completes it.
#[derive(Debug, Clone, Default)]
pub(crate) struct GhSnapshot {
    pub snap: PrSnapshot,
    pub commit_logins: HashMap<String, String>,
}

/// Go `parseGitHubPRView` (the `gh pr view --json` shape).
#[cfg(test)]
pub(crate) fn parse_view(raw: &str, now: DateTime<Utc>) -> Result<GhSnapshot, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty gh pr view json".into());
    }
    let view: ViewJson = decode(raw).map_err(|e| format!("parse gh pr view json: {e}"))?;
    Ok(parse_view_json(view, now))
}

/// Go `parseGitHubPRViewJSON`.
pub(crate) fn parse_view_json(view: ViewJson, now: DateTime<Utc>) -> GhSnapshot {
    let url = view.url.trim().to_owned();
    let commit_logins = commit_logins(&view.commits);
    let mut snap = PrSnapshot {
        title: view.title.trim().to_owned(),
        state: view.state.trim().to_uppercase(),
        review_decision: view.review_decision.trim().to_uppercase(),
        mergeable: view.mergeable.trim().to_uppercase(),
        merge_state_status: view.merge_state_status.trim().to_uppercase(),
        is_draft: view.is_draft,
        head_sha: view.head_ref_oid.trim().to_owned(),
        head_ref: view.head_ref_name.trim().to_owned(),
        base_ref: view.base_ref_name.trim().to_owned(),
        url: url.clone(),
        author_login: view.author.login.trim().to_owned(),
        node_id: view.id.trim().to_owned(),
        created_at: view.created_at.trim().to_owned(),
        updated_at: view.updated_at.trim().to_owned(),
        review_requests: view
            .review_requests
            .iter()
            .map(ListedActor::participant)
            .collect(),
        assignees: view
            .assignees
            .iter()
            .map(ListedActor::participant)
            .collect(),
        commits: commits_from_view(&view.commits, now),
        commit_authors: commit_logins
            .iter()
            .map(|(oid, login)| {
                (
                    oid.clone(),
                    CommitAuthor {
                        name: display_name(login),
                        avatar_url: avatar_url(login, "", 56),
                    },
                )
            })
            .collect(),
        ..Default::default()
    };
    let mut opened = view.created_at.trim().to_owned();
    if opened.is_empty() {
        opened = format_time(now);
    }
    snap.comments.push(description_comment(
        &view.author,
        &view.body,
        &opened,
        &url,
        &view.id,
        now,
    ));
    for c in &view.comments {
        let mut id = c.id.trim().to_owned();
        if id.is_empty() {
            id = format!("{}-{}", c.created_at, c.author.login);
        }
        let mut row = thread_comment(
            &format!("gh-comment-{id}"),
            &c.author,
            &c.body,
            "",
            &c.created_at,
            now,
            "comment",
        );
        row.url = permalink(&url, "issuecomment", &id, &c.url);
        row.node_id = id;
        snap.comments.push(row);
    }
    for r in &view.reviews {
        if r.state.eq_ignore_ascii_case("PENDING") {
            continue;
        }
        let mut id = r.id.trim().to_owned();
        if id.is_empty() {
            id = format!("{}-{}-{}", r.submitted_at, r.author.login, r.state);
        }
        let mut when = r.submitted_at.clone();
        if when.is_empty() {
            when = format_time(now);
        }
        let link = permalink(&url, "pullrequestreview", &id, &r.url);
        let body = r.body.trim();
        let mut row = if body.is_empty() {
            action_comment(
                &format!("gh-review-action-{id}"),
                &r.author,
                review_action(&r.state),
                &when,
                now,
            )
        } else {
            thread_comment(
                &format!("gh-review-{id}"),
                &r.author,
                body,
                "",
                &when,
                now,
                "comment",
            )
        };
        row.node_id = id;
        row.url = link;
        row.verb = review_card_verb(&r.state).to_owned();
        snap.comments.push(row);
    }
    GhSnapshot {
        snap,
        commit_logins,
    }
}

/// Go `parseGitHubInlineComments`.
pub(crate) fn parse_inline_comments(raw: &str, now: DateTime<Utc>, pr_url: &str) -> Vec<PrComment> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "[]" {
        return Vec::new();
    }
    match decode::<Vec<InlineComment>>(raw) {
        Ok(rows) => comments_from_inline_rows(&rows, now, pr_url),
        Err(_) => Vec::new(),
    }
}

/// Go `commentsFromInlineRows`.
pub(crate) fn comments_from_inline_rows(
    rows: &[InlineComment],
    now: DateTime<Utc>,
    pr_url: &str,
) -> Vec<PrComment> {
    let base = pr_url.trim().trim_end_matches('/');
    rows.iter()
        .map(|row| {
            let path = row.path.trim();
            let line = row.comment_line();
            let start = row.comment_start_line();
            let outdated = row.outdated();
            let mut loc = path.to_owned();
            if !loc.is_empty() && line > 0 {
                loc = if start > 0 {
                    format!("{loc}:{start}-{line}")
                } else {
                    format!("{loc}:{line}")
                };
            }
            if outdated && !loc.is_empty() {
                loc.push_str(" (outdated)");
            }
            let mut c = thread_comment(
                &format!("gh-inline-{}", row.id),
                &row.user,
                &row.body,
                &loc,
                &row.created_at,
                now,
                "comment",
            );
            c.url = row.html_url.trim().to_owned();
            if c.url.is_empty() && !base.is_empty() && row.id > 0 {
                c.url = format!("{base}#discussion_r{}", row.id);
            }
            c.path = path.to_owned();
            c.line = line;
            c.start_line = start;
            c.side = row.side.trim().to_owned();
            c.start_side = row.start_side.trim().to_owned();
            c.outdated = outdated;
            c.diff_hunk = row.diff_hunk.trim().to_owned();
            if row.pull_request_review_id > 0 {
                c.review_id = row.pull_request_review_id.to_string();
            }
            if row.in_reply_to_id > 0 {
                c.in_reply_to_id = format!("gh-inline-{}", row.in_reply_to_id);
            }
            c
        })
        .collect()
}

/// Go `parsePostedInlineComment`: the comment a reply POST returned.
pub(crate) fn parse_posted_inline_comment(
    raw: &str,
    now: DateTime<Utc>,
    pr_url: &str,
) -> Option<PrComment> {
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return None;
    }
    let mut cs = parse_inline_comments(&format!("[{raw}]"), now, pr_url);
    if cs.len() != 1 || cs[0].id.trim().is_empty() {
        return None;
    }
    cs.pop()
}

/// The GitHub permalink for a conversation row: `existing` when set, else
/// `<pr>#<fragment>-<database id>` derived from the node id.
pub(crate) fn permalink(pr_url: &str, fragment: &str, node_id: &str, existing: &str) -> String {
    let existing = existing.trim();
    if !existing.is_empty() {
        return existing.to_owned();
    }
    let base = pr_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return String::new();
    }
    match node_database_id(node_id) {
        Some(db) => format!("{base}#{fragment}-{db}"),
        None => String::new(),
    }
}

/// The REST database id inside a GitHub id: numeric ids as is, next-gen node
/// ids (`PRR_…`, msgpack payload) and legacy base64 `NNN:TypeNNN` ids.
pub fn node_database_id(id: &str) -> Option<i64> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    if let Ok(n) = id.parse::<i64>()
        && n > 0
    {
        return Some(n);
    }
    if let Some(i) = id.find('_')
        && i > 0
        && i < id.len() - 1
        && node_prefix(&id[..i])
    {
        let raw = decode_node_payload(&id[i + 1..])?;
        if raw.len() < 5 {
            return None;
        }
        let mut n: u64 = 0;
        for b in &raw[4..] {
            n = (n << 8) | u64::from(*b);
        }
        return (n > 0).then_some(n as i64);
    }
    legacy_node_id(&decode_node_payload(id)?)
}

fn node_prefix(s: &str) -> bool {
    !s.is_empty() && s.len() <= 8 && s.bytes().all(|c| c.is_ascii_uppercase())
}

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_allow_trailing_bits(true)
    .with_decode_padding_mode(DecodePaddingMode::RequireCanonical);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::URL_SAFE, LENIENT);
const STANDARD: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::STANDARD, LENIENT);

fn decode_node_payload(payload: &str) -> Option<Vec<u8>> {
    let mut payload = payload.trim().to_owned();
    if payload.is_empty() {
        return None;
    }
    let m = payload.len() % 4;
    if m != 0 {
        payload.push_str(&"=".repeat(4 - m));
    }
    if let Ok(raw) = URL_SAFE.decode(&payload)
        && !raw.is_empty()
    {
        return Some(raw);
    }
    STANDARD.decode(&payload).ok().filter(|r| !r.is_empty())
}

fn legacy_node_id(raw: &[u8]) -> Option<i64> {
    let i = raw.iter().position(|&b| b == b':')?;
    if i == raw.len() - 1 {
        return None;
    }
    let rest = &raw[i + 1..];
    let j = rest.iter().take_while(|b| b.is_ascii_alphabetic()).count();
    if j == 0 || j == rest.len() {
        return None;
    }
    let n: i64 = std::str::from_utf8(&rest[j..]).ok()?.parse().ok()?;
    (n > 0).then_some(n)
}

fn description_comment(
    author: &Actor,
    body: &str,
    when: &str,
    pr_url: &str,
    node_id: &str,
    now: DateTime<Utc>,
) -> PrComment {
    let mut c = thread_comment("gh-description", author, body, "", when, now, "description");
    c.node_id = node_id.trim().to_owned();
    c.url = permalink(pr_url, "issue", node_id, "");
    if c.url.is_empty() {
        c.url = pr_url.trim().to_owned();
    }
    if body.trim().is_empty() {
        c.body_md.clear();
    }
    c
}

pub(crate) fn action_comment(
    id: &str,
    author: &Actor,
    verb: &str,
    when: &str,
    now: DateTime<Utc>,
) -> PrComment {
    thread_comment(id, author, verb, "", when, now, "action")
}

/// Go `ghThreadComment`.
pub(crate) fn thread_comment(
    id: &str,
    author: &Actor,
    body: &str,
    loc: &str,
    when: &str,
    now: DateTime<Utc>,
    kind: &str,
) -> PrComment {
    let mut login = author.login.trim();
    if login.is_empty() {
        login = "someone";
    }
    let display = display_name(login);
    let kind = if kind.is_empty() { "comment" } else { kind };
    PrComment {
        id: id.to_owned(),
        code: author_code(&display),
        author_login: if login.eq_ignore_ascii_case("someone") {
            String::new()
        } else {
            login.to_owned()
        },
        author: display,
        avatar_css: comment_avatar_css("AR").to_owned(),
        time: format_when(when, now),
        at: when.trim().to_owned(),
        kind: kind.to_owned(),
        verb: if kind == "comment" {
            "commented".to_owned()
        } else {
            String::new()
        },
        body_md: body.trim().to_owned(),
        body_html: if loc.is_empty() {
            String::new()
        } else {
            format!("<code>{}</code>", crate::model::html_escape(loc))
        },
        avatar_url: avatar_url(login, author.avatar(), 56),
        ..Default::default()
    }
}

/// Records an actor's API avatar under its login, display name and
/// `[bot]`-less login.
pub(crate) fn remember_avatar(m: &mut HashMap<String, String>, a: &Actor) {
    let url = a.avatar();
    let login = a.login.trim();
    if login.is_empty() || url.is_empty() {
        return;
    }
    m.insert(login.to_lowercase(), url.to_owned());
    let display = display_name(login).to_lowercase();
    if !display.is_empty() {
        m.insert(display, url.to_owned());
    }
    let trimmed = login.strip_suffix("[bot]").unwrap_or(login).to_lowercase();
    if !trimmed.is_empty() {
        m.insert(trimmed, url.to_owned());
    }
}

/// Upgrades rows that show an avatar to the API avatar for their author.
pub(crate) fn apply_avatars(comments: &mut [PrComment], avatars: &HashMap<String, String>) {
    if avatars.is_empty() {
        return;
    }
    for c in comments {
        if c.avatar_url.trim().is_empty() {
            continue;
        }
        let hit = [&c.author_login, &c.author]
            .into_iter()
            .find_map(|k| {
                avatars
                    .get(&k.trim().to_lowercase())
                    .filter(|u| !u.is_empty())
            })
            .cloned();
        if let Some(url) = hit {
            c.avatar_url = url;
        }
    }
}

/// Two-letter avatar code for a display name; `GH` when empty.
pub(crate) fn author_code(login: &str) -> String {
    let r: Vec<char> = login.trim().to_uppercase().chars().collect();
    match r.len() {
        0 => "GH".to_owned(),
        1 => r[0].to_string(),
        _ => r[..2].iter().collect(),
    }
}

pub(crate) fn review_action(state: &str) -> &'static str {
    match state.trim().to_uppercase().as_str() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "dismissed a review",
        _ => "reviewed",
    }
}

pub(crate) fn review_card_verb(state: &str) -> &'static str {
    match state.trim().to_uppercase().as_str() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "dismissed a review",
        _ => "commented",
    }
}

/// Commit oid → the first linked GitHub login among its authors.
pub(crate) fn commit_logins(commits: &[ViewCommit]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for c in commits {
        let oid = c.oid.trim();
        if oid.is_empty() {
            continue;
        }
        if let Some(login) = c
            .authors
            .iter()
            .map(|a| a.login.trim())
            .find(|l| !l.is_empty())
        {
            m.insert(oid.to_owned(), login.to_owned());
        }
    }
    m
}

/// Go `commitsFromGitHubView`: commits in forge order with display authors.
pub(crate) fn commits_from_view(rows: &[ViewCommit], now: DateTime<Utc>) -> Vec<Commit> {
    rows.iter()
        .filter_map(|row| {
            let oid = row.oid.trim();
            if oid.is_empty() {
                return None;
            }
            let mut author = String::new();
            for a in &row.authors {
                let login = a.login.trim();
                if !login.is_empty() {
                    author = display_name(login);
                    break;
                }
                let name = a.name.trim();
                if !name.is_empty() {
                    author = name.to_owned();
                    break;
                }
            }
            Some(Commit {
                hash: oid.chars().take(7).collect(),
                sha: oid.to_owned(),
                message: row.message_headline.trim().to_owned(),
                body: row.message_body.trim().to_owned(),
                author,
                when: compact_ago(parse_time(&row.committed_date), now),
                ..Default::default()
            })
        })
        .collect()
}

/// Commit metadata and actor avatars from the snapshot (or rollup) GraphQL.
pub(crate) fn parse_commit_rollup(
    raw: &str,
) -> (HashMap<String, CommitMeta>, HashMap<String, String>) {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Payload {
        data: Data,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Data {
        repository: Repo,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Repo {
        pull_request: Pull,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Pull {
        author: Actor,
        comments: Nodes<Authored>,
        reviews: Nodes<Authored>,
        commits: Nodes<CommitNode>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Authored {
        author: Actor,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct CommitNode {
        commit: CommitBody,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct CommitBody {
        oid: String,
        url: String,
        signature: Option<Sig>,
        status_check_rollup: Option<Rollup>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Sig {
        is_valid: bool,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Rollup {
        state: String,
    }

    let raw = raw.trim();
    let mut meta = HashMap::new();
    let mut avatars = HashMap::new();
    if raw.is_empty() {
        return (meta, avatars);
    }
    let Ok(p) = decode::<Payload>(raw) else {
        return (meta, avatars);
    };
    let pr = p.data.repository.pull_request;
    remember_avatar(&mut avatars, &pr.author);
    for n in pr.comments.nodes.iter().chain(&pr.reviews.nodes) {
        remember_avatar(&mut avatars, &n.author);
    }
    for n in pr.commits.nodes {
        let oid = n.commit.oid.trim();
        if oid.is_empty() {
            continue;
        }
        meta.insert(
            oid.to_owned(),
            CommitMeta {
                verified: n.commit.signature.is_some_and(|s| s.is_valid),
                check_status: n
                    .commit
                    .status_check_rollup
                    .map(|r| rollup_status(&r.state).to_owned())
                    .unwrap_or_default(),
                url: n.commit.url.trim().to_owned(),
            },
        );
    }
    (meta, avatars)
}

/// A commit's combined status as a check bucket.
pub(crate) fn rollup_status(state: &str) -> &'static str {
    match state.trim().to_uppercase().as_str() {
        "SUCCESS" => "pass",
        "FAILURE" | "ERROR" => "fail",
        "PENDING" | "EXPECTED" => "pending",
        _ => "",
    }
}

/// Go `parseMentionableUsers`.
pub(crate) fn parse_mentionable_users(raw: &str) -> Vec<PrParticipant> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Payload {
        data: Data,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Data {
        repository: Repo,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Repo {
        mentionable_users: Users,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Users {
        nodes: Vec<User>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct User {
        login: String,
        name: String,
        avatar_url: String,
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let Ok(p) = decode::<Payload>(raw) else {
        return Vec::new();
    };
    p.data
        .repository
        .mentionable_users
        .nodes
        .into_iter()
        .filter_map(|n| {
            let login = n.login.trim();
            (!login.is_empty()).then(|| PrParticipant {
                login: login.to_owned(),
                name: n.name.trim().to_owned(),
                avatar_url: avatar_url(login, n.avatar_url.trim(), 56),
                ..Default::default()
            })
        })
        .collect()
}

/// Nests line notes under the review that submitted them (GitHub's
/// Conversation tab), points replies at their thread root, promotes reviews
/// that gained children to cards, and hides review actions that only carry
/// replies.
pub(crate) fn attach_review_comments(cs: &mut [PrComment]) {
    let mut parents: HashMap<String, usize> = HashMap::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (i, c) in cs.iter().enumerate() {
        by_id.insert(c.id.clone(), i);
        if let Some(rest) = c.id.strip_prefix("gh-inline-") {
            by_id.insert(rest.to_owned(), i);
        }
        if !c.id.starts_with("gh-review-") {
            continue;
        }
        for key in review_match_keys(c) {
            parents.insert(key, i);
        }
    }
    let mut has_kids: Vec<usize> = Vec::new();
    for i in 0..cs.len() {
        if !cs[i].id.starts_with("gh-inline-") || !cs[i].in_reply_to_id.is_empty() {
            continue;
        }
        let Some(&pi) = parents.get(cs[i].review_id.trim()) else {
            continue;
        };
        cs[i].parent_id = cs[pi].id.clone();
        if !has_kids.contains(&pi) {
            has_kids.push(pi);
        }
    }
    for i in 0..cs.len() {
        if !cs[i].id.starts_with("gh-inline-") || cs[i].in_reply_to_id.is_empty() {
            continue;
        }
        let root = resolve_inline_root(&by_id, cs, &cs[i].in_reply_to_id.clone());
        if !root.is_empty() {
            let parent = by_id.get(&root).map(|&ri| cs[ri].parent_id.clone());
            cs[i].in_reply_to_id = root;
            if let Some(parent) = parent {
                cs[i].parent_id = parent;
            }
        }
    }
    for pi in has_kids {
        promote_review_with_kids(cs, pi);
    }
    hide_reply_only_review_actions(cs);
}

fn resolve_inline_root(by_id: &HashMap<String, usize>, cs: &[PrComment], id: &str) -> String {
    let mut seen = HashSet::new();
    let mut cur = id.trim().to_owned();
    while !cur.is_empty() && seen.insert(cur.clone()) {
        let found = by_id.get(&cur).or_else(|| {
            let bare = cur.strip_prefix("gh-inline-").unwrap_or(&cur);
            by_id.get(&format!("gh-inline-{bare}"))
        });
        let Some(&i) = found else {
            if cur.starts_with("gh-inline-") {
                return cur;
            }
            return format!("gh-inline-{cur}");
        };
        if cs[i].in_reply_to_id.is_empty() {
            return cs[i].id.clone();
        }
        cur = cs[i].in_reply_to_id.clone();
    }
    cur
}

fn hide_reply_only_review_actions(cs: &mut [PrComment]) {
    let mut has_root = HashSet::new();
    let mut has_reply = HashSet::new();
    for c in cs.iter() {
        if !c.id.starts_with("gh-inline-") || c.review_id.is_empty() {
            continue;
        }
        if c.in_reply_to_id.is_empty() {
            has_root.insert(c.review_id.clone());
        } else {
            has_reply.insert(c.review_id.clone());
        }
    }
    for c in cs.iter_mut() {
        if c.kind != "action" || !c.id.starts_with("gh-review-") {
            continue;
        }
        if c.verb != "commented" && c.body_md != "reviewed" {
            continue;
        }
        let mut only_replies = false;
        for key in review_match_keys(c) {
            if has_root.contains(&key) {
                only_replies = false;
                break;
            }
            if has_reply.contains(&key) {
                only_replies = true;
            }
        }
        if only_replies {
            c.kind = "hidden".to_owned();
        }
    }
}

/// Ids a review row can be referenced by: its node id, database id, and the
/// id in its `pullrequestreview-N` permalink.
fn review_match_keys(c: &PrComment) -> Vec<String> {
    let raw = c.id.strip_prefix("gh-review-").unwrap_or(&c.id);
    let raw = raw.strip_prefix("action-").unwrap_or(raw);
    let mut keys = Vec::with_capacity(3);
    if !raw.is_empty() {
        keys.push(raw.to_owned());
    }
    if let Some(db) = node_database_id(raw) {
        keys.push(db.to_string());
    }
    let u = c.url.trim();
    if let Some(i) = u.rfind("pullrequestreview-") {
        let mut n = &u[i + "pullrequestreview-".len()..];
        if let Some(j) = n.find(['?', '#', '&']) {
            n = &n[..j];
        }
        if !n.is_empty() {
            keys.push(n.to_owned());
        }
    }
    keys
}

fn promote_review_with_kids(cs: &mut [PrComment], pi: usize) {
    if cs[pi].kind != "action" {
        return;
    }
    let old = cs[pi].id.clone();
    if let Some(rest) = old.strip_prefix("gh-review-action-") {
        let new = format!("gh-review-{rest}");
        cs[pi].id = new.clone();
        for c in cs.iter_mut() {
            if c.parent_id == old {
                c.parent_id = new.clone();
            }
        }
    }
    let c = &mut cs[pi];
    if c.verb.trim().is_empty() {
        c.verb = review_action_card_verb(&c.body_md).to_owned();
    }
    c.kind = "comment".to_owned();
    c.body_md.clear();
    c.body_html.clear();
}

fn review_action_card_verb(body: &str) -> &'static str {
    match body.trim() {
        "approved" => "approved",
        "requested changes" => "requested changes",
        "dismissed a review" => "dismissed a review",
        _ => "commented",
    }
}

/// One review thread: its node id, resolution and member comment ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewThread {
    pub id: String,
    pub resolved: bool,
    pub comment_ids: Vec<i64>,
}

/// Go `parseGitHubReviewThreads`.
pub(crate) fn parse_review_threads(raw: &str) -> Vec<ReviewThread> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Payload {
        data: Data,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Data {
        repository: Repo,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Repo {
        pull_request: Pull,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Pull {
        review_threads: Nodes<Thread>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Thread {
        id: String,
        is_resolved: bool,
        comments: Nodes<Member>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Member {
        database_id: i64,
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let Ok(p) = decode::<Payload>(raw) else {
        return Vec::new();
    };
    p.data
        .repository
        .pull_request
        .review_threads
        .nodes
        .into_iter()
        .filter_map(|n| {
            let id = n.id.trim();
            (!id.is_empty()).then(|| ReviewThread {
                id: id.to_owned(),
                resolved: n.is_resolved,
                comment_ids: n
                    .comments
                    .nodes
                    .iter()
                    .map(|c| c.database_id)
                    .filter(|&d| d > 0)
                    .collect(),
            })
        })
        .collect()
}

/// Stamps thread id and resolution onto the line notes in each thread.
pub(crate) fn apply_review_threads(cs: &mut [PrComment], threads: &[ReviewThread]) {
    if threads.is_empty() {
        return;
    }
    let mut by_comment: HashMap<String, &ReviewThread> = HashMap::new();
    for t in threads {
        for id in &t.comment_ids {
            by_comment.insert(format!("gh-inline-{id}"), t);
        }
    }
    for c in cs {
        if let Some(t) = by_comment.get(&c.id) {
            c.thread_id = t.id.clone();
            c.resolved = t.resolved;
        }
    }
}

/// Description first, then by timestamp. Rows without a parseable timestamp
/// go last, in their original order: their age is unknown, and trailing them
/// keeps every dated row in time order.
pub(crate) fn sort_comments(cs: &mut [PrComment]) {
    cs.sort_by_cached_key(|c| {
        let at = parse_time(&c.at);
        (c.kind != "description", at.is_none(), at)
    });
}

/// Go `mergeGitHubComments`: the snapshot's rows, then stored rows it does
/// not have that must survive: Recur-only rows, older comments/reviews when
/// the snapshot was truncated, and just-posted line notes GitHub has not
/// echoed yet.
pub(crate) fn merge_comments(
    existing: &[PrComment],
    from_gh: &[PrComment],
    keep_comments: bool,
    keep_reviews: bool,
    now: DateTime<Utc>,
) -> Vec<PrComment> {
    let have: HashSet<&str> = from_gh.iter().map(|c| c.id.as_str()).collect();
    let mut out = from_gh.to_vec();
    for c in existing {
        if have.contains(c.id.as_str()) {
            continue;
        }
        let keep = !c.id.starts_with("gh-")
            || (keep_comments && c.id.starts_with("gh-comment-"))
            || (keep_reviews && c.id.starts_with("gh-review-"))
            || keep_fresh_inline(c, now);
        if keep {
            out.push(c.clone());
        }
    }
    out
}

fn keep_fresh_inline(c: &PrComment, now: DateTime<Utc>) -> bool {
    if !c.id.starts_with("gh-inline-") {
        return false;
    }
    if c.time == "now" {
        return true;
    }
    parse_time(&c.at).is_some_and(|t| now - t < chrono::TimeDelta::minutes(2))
}
