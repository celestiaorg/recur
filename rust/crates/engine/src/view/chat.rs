//! The agent-chat transcript rules shared by every chat surface (frontend
//! `lib/agentchat.ts`): mapping persisted `ChatItem`s to rows, merging a live
//! stream onto a store snapshot, the live reducers (stream, thinking, tools,
//! compaction, fallback, notices, footers), the turn-clock gates, and the
//! chat sentinels and error-card wording. Pure list transforms: the view
//! owns the per-workspace lists and streaming flags and calls these.
//!
//! Unset TS fields are the Rust defaults (`""`, `false`, `0`, empty vec);
//! `ts` stays an `Option` because `??` and `||` treat a stored `0` and an
//! absent stamp differently.

use std::collections::HashSet;
use std::sync::LazyLock;

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent::error::CODEX_LOGIN_EXPIRED;
use recur_domain::ChatItem;
use regex::Regex;

use super::js;
use super::mentions::{extract_attachment_ids, same_user_chat_body};

/// An image shown on a user bubble.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAttachmentView {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub url: String,
    pub data_url: String,
}

/// A pasted image waiting to be sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAttachmentDraft {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub data_url: String,
}

/// One specialist session in the Loop cockpit log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatLogSession {
    pub id: String,
    pub code: String,
    pub name: String,
    pub messages: Vec<AgentMsg>,
    pub live: bool,
}

/// A cockpit log row carried by a transcript entry.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatLog {
    Fact {
        id: String,
        text: String,
        is_deviation: bool,
        code: String,
        name: String,
        /// A typed card ("fanout") rendered from `data` instead of text.
        card: String,
        data: String,
    },
    Session(ChatLogSession),
    /// Consecutive specialist rows packed into one fanout card.
    Sessions {
        id: String,
        sessions: Vec<ChatLogSession>,
    },
}

/// One transcript row (frontend `AgentMsg`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentMsg {
    pub body: String,
    pub me: bool,
    pub note: bool,
    /// Notice row label; "" renders "Notice".
    pub note_label: String,
    /// Engine-fact markup (commit/handoff); ordinary notices stay escaped.
    pub note_html: bool,
    pub streaming: bool,
    /// A trailing store-mapped assistant row whose `streaming` a remount
    /// lost: [`apply_stream`] may widen it; a non-prefix snapshot opens a
    /// new row.
    pub widen_stream: bool,
    pub tool: String,
    pub tool_input: String,
    pub tool_id: String,
    pub description: String,
    pub input: String,
    pub result: String,
    pub result_error: bool,
    pub result_truncated: bool,
    pub result_pending: bool,
    pub turn_end: bool,
    pub duration_ms: i64,
    pub message_id: String,
    /// Turn footer: this turn changed files.
    pub checkpoint: bool,
    pub stopped: bool,
    pub error: bool,
    pub thinking: bool,
    pub tokens: i64,
    pub compact: bool,
    pub compacting: bool,
    pub fallback: bool,
    pub provider_uuid: String,
    pub attachments: Vec<ChatAttachmentView>,
    pub queued: bool,
    pub pending_id: String,
    pub optimistic: bool,
    pub ts: Option<i64>,
    pub log: Option<Box<ChatLog>>,
    /// Nested subagent output rendered inside the parent Task card.
    pub children: Vec<AgentMsg>,
}

/// A live `session.step` (frontend `WorkspaceStep`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceStep {
    pub id: String,
    pub tool: String,
    pub tool_input: String,
    pub description: String,
    pub input: String,
    /// The provider tool_use id of a parent Task for a nested subagent call.
    pub parent_id: String,
    pub uuid: String,
    pub owner_code: String,
}

fn has(s: &str) -> bool {
    !s.is_empty()
}

fn ts_truthy(ts: Option<i64>) -> bool {
    ts.is_some_and(|t| t != 0)
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Stable prefix of backend-synthesized chat failure replies (matches Go).
pub const AGENT_ERROR_PREFIX: &str = "Agent error:";

/// The durable note when the user interrupts an in-flight turn.
pub const CHAT_STOPPED_REPLY: &str = "Stopped by user";

/// The UI-only display_text Retry sends (Go `chatRetryDisplaySentinel`), so
/// a retry adds no second user bubble.
pub const CHAT_RETRY_DISPLAY: &str = crate::codes::CHAT_RETRY_DISPLAY_SENTINEL;

/// The send reply when a follow-up was injected into a live turn (Go
/// `chatInjectedSentinel`): it must not finish or idle the tab.
pub const CHAT_INJECTED_REPLY: &str = crate::turn::finalize::CHAT_INJECTED_SENTINEL;

const GENERIC_AGENT_ERROR_REPLIES: [&str; 2] = [
    "I hit an error trying to respond just now. Try again.",
    "The agent hit an error — try again.",
];

const CODER_FIX_DISPLAY: &str = crate::roles::CODER_FIX_DISPLAY;
const CODER_RESUME_DISPLAY: &str = "Resume coding";

pub fn is_chat_injected_reply(text: &str) -> bool {
    text == CHAT_INJECTED_REPLY
}

/// The user-interrupt note (current or legacy wording).
pub fn is_chat_stopped_reply(text: &str) -> bool {
    let t = js::trim(text);
    t == CHAT_STOPPED_REPLY || t == "Stopped."
}

/// A provider/engine failure note to keep visible.
pub fn is_agent_error_reply(text: &str) -> bool {
    let t = js::trim(text);
    t.starts_with(AGENT_ERROR_PREFIX) || GENERIC_AGENT_ERROR_REPLIES.contains(&t)
}

static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("sgr re"));
static C0: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]").expect("c0 re"));

// simplify: SGR/C0 only; OSC/DCS stay in term.StripVT. Upgrade if a CLI dumps those into chat.
fn strip_vt(s: &str) -> String {
    C0.replace_all(&SGR.replace_all(s, ""), "").into_owned()
}

fn known_agent_error(s: &str) -> &'static str {
    let d = s.to_lowercase();
    if d.contains("failed to refresh token") || (d.contains("codex_login") && d.contains("401")) {
        CODEX_LOGIN_EXPIRED
    } else {
        ""
    }
}

const LOG_LEVELS: [&str; 6] = [
    " ERROR ",
    " WARN ",
    " WARNING ",
    " INFO ",
    " DEBUG ",
    " TRACE ",
];

fn strip_cli_log_prefix(s: &str) -> &str {
    let Some(i) = s.find(" - ").filter(|&i| i > 0) else {
        return s;
    };
    let head = &s[..i];
    let u = format!(" {} ", head.to_uppercase());
    if !LOG_LEVELS.iter().any(|l| u.contains(l)) {
        return s;
    }
    let stamped = head.contains('T') && head.contains('Z');
    if !head.contains("::") && !stamped {
        return s;
    }
    js::trim(&s[i + 3..])
}

fn tidy_agent_error(s: &str) -> String {
    let s = strip_vt(s);
    let known = known_agent_error(&s);
    if !known.is_empty() {
        return known.to_owned();
    }
    let stripped = strip_cli_log_prefix(&s);
    if stripped != s {
        let cut = match stripped.find('{') {
            Some(b) if b > 0 => &stripped[..b],
            _ => stripped,
        };
        return js::trim(cut).to_owned();
    }
    js::trim(&s).to_owned()
}

/// The provider/engine detail with the "Agent error:" prefix stripped.
pub fn agent_error_detail(text: &str) -> String {
    let mut t = strip_vt(js::trim(text));
    if let Some(rest) = t.strip_prefix(AGENT_ERROR_PREFIX) {
        t = js::trim(rest).to_owned();
    }
    tidy_agent_error(&t)
}

static USAGE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"spend limit|usage limit|usage_limit|usage-credits|credit balance|insufficient credit|(?-u:\b)quota(?-u:\b)|(?-u:\b)billing(?-u:\b)|upgrade your plan")
        .expect("usage re")
});
static RATE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"rate limit|rate_limit|too many requests|(?-u:\b)429(?-u:\b)").expect("rate re")
});
static CODEX_LOGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("codex login|refresh token|login expired").expect("login re"));

/// The card title of a failed turn: usage or rate limit when the provider
/// said so.
pub fn agent_error_title(text: &str) -> &'static str {
    let d = agent_error_detail(text).to_lowercase();
    if USAGE_LIMIT.is_match(&d) {
        "Usage limit"
    } else if RATE_LIMIT.is_match(&d) {
        "Rate limit"
    } else if CODEX_LOGIN.is_match(&d) {
        "Codex login"
    } else {
        "Agent error"
    }
}

fn same_error_detail(a: &str, b: &str) -> bool {
    agent_error_detail(a).to_lowercase() == agent_error_detail(b).to_lowercase()
}

fn is_retry_display(text: &str) -> bool {
    text == CHAT_RETRY_DISPLAY
}

fn is_mode_switch_continue_display(text: &str) -> bool {
    let t = js::trim(text);
    t.starts_with("Switched to ") && t.ends_with(": continuing")
}

static HTML_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[a-zA-Z][a-zA-Z0-9]*(?-u:\b)[^>]*>").expect("tag re"));
static WS_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[{}]+", js::WS_CLASS)).expect("ws re"));

/// The prose form of an ask_user/escalation option for the engine `choice`:
/// real tags are stripped (so `count < 5` survives) and whitespace folded.
pub fn option_choice_text(s: &str) -> String {
    let stripped = HTML_TAG.replace_all(s, "");
    js::trim(&WS_RUN.replace_all(&stripped, " ")).to_owned()
}

/// The short bubble label of an ask_user answer (Go `formatAskUserDisplay`).
pub fn format_ask_user_display_text(choice: &str, free_text: &str) -> String {
    let c = js::trim(choice);
    let f = js::trim(free_text);
    match (c.is_empty(), f.is_empty()) {
        (false, false) => format!("{c} — {f}"),
        (false, true) => c.to_owned(),
        (true, false) => f.to_owned(),
        (true, true) => "(empty)".to_owned(),
    }
}

static ASK_USER_RESUME: LazyLock<Regex> = LazyLock::new(|| {
    let ws = js::WS_CLASS;
    let dot = r"[^\n\r\x{2028}\x{2029}]";
    Regex::new(&format!(
        r"^The user answered your earlier ask_user question\.[{ws}]*\n[\s\S]*?\nAnswer:[{ws}]*({dot}+?)(?:\nDetails:[{ws}]*({dot}+?))?(?:\n\nContinue with this answer[\s\S]*)?$"
    ))
    .expect("ask resume re")
});

/// A user row's bubble text: ask_user resume wire collapses to the answer and
/// Coder dispatch wire to its label (rows persisted without display_text).
pub fn display_user_chat_text(text: &str) -> String {
    if let Some(c) = ASK_USER_RESUME.captures(text) {
        return format_ask_user_display_text(
            c.get(1).map_or("", |m| m.as_str()),
            c.get(2).map_or("", |m| m.as_str()),
        );
    }
    if text.starts_with("You are the Coder agent. A reviewer found issues")
        || text.starts_with("You stopped while reviewer findings")
    {
        return CODER_FIX_DISPLAY.to_owned();
    }
    if text.starts_with("You were interrupted mid-task")
        || text.starts_with("You are the Coder agent resuming an interrupted task")
    {
        return CODER_RESUME_DISPLAY.to_owned();
    }
    text.to_owned()
}

/// The bubble of a queued or flushed send: display_text when set, else the
/// marked wire (so attachment markers align with `session.user`).
pub fn pending_send_bubble_body(display_text: &str, marked: &str) -> String {
    let d = js::trim(display_text);
    display_user_chat_text(if d.is_empty() { marked } else { d })
}

fn user_turn_count(list: &[AgentMsg]) -> usize {
    list.iter().filter(|m| m.me && !m.queued).count()
}

fn plain(m: &AgentMsg) -> bool {
    !m.me && !has(&m.tool) && !m.thinking && !m.compact && !m.note && !m.fallback
}

/// Whether a persisted and a live row are the same turn item for alignment.
/// Optimistic live users never match (a resend is a new turn); streaming
/// rows are live-only; tools match on name+input so a live result can
/// overlay a still-pending snapshot.
fn row_equal(p: &AgentMsg, l: &AgentMsg) -> bool {
    if l.queued || p.queued || (l.optimistic && l.me) || l.streaming || p.streaming {
        return false;
    }
    if p.me && l.me {
        return p.body == l.body;
    }
    if has(&p.tool) && has(&l.tool) {
        return p.tool == l.tool && p.tool_input == l.tool_input;
    }
    if p.thinking && l.thinking {
        return p.body == l.body;
    }
    if p.compact && l.compact {
        return p.body == l.body && p.compacting == l.compacting;
    }
    if p.note || l.note {
        return p.note && l.note && p.body == l.body;
    }
    if p.fallback && l.fallback {
        return p.body == l.body && p.description == l.description;
    }
    if p.turn_end || l.turn_end {
        return p.turn_end && l.turn_end;
    }
    if plain(p) && plain(l) {
        return p.body == l.body;
    }
    false
}

fn is_prefix(suffix: &[AgentMsg], live: &[AgentMsg]) -> bool {
    suffix.len() <= live.len() && suffix.iter().zip(live).all(|(s, l)| row_equal(s, l))
}

/// The earliest persisted index whose suffix is a prefix of `live`. An empty
/// suffix always matches, so a disjoint tail is appended.
fn live_align_start(persisted: &[AgentMsg], live: &[AgentMsg]) -> usize {
    (0..=persisted.len())
        .find(|&start| is_prefix(&persisted[start..], live))
        .unwrap_or(persisted.len())
}

fn common_prefix_len(persisted: &[AgentMsg], live: &[AgentMsg]) -> usize {
    persisted
        .iter()
        .zip(live)
        .take_while(|(p, l)| row_equal(p, l))
        .count()
}

fn overlay_match(p: &AgentMsg, e: &AgentMsg) -> bool {
    row_equal(p, e) || (e.me && e.optimistic && p.me && same_user_chat_body(&e.body, &p.body))
}

fn overlay_aligned(persisted: &[AgentMsg], extra: &[AgentMsg], from: usize) -> Vec<AgentMsg> {
    let mut out = persisted.to_vec();
    let mut p = from;
    let mut i = 0;
    while i < extra.len() && p < out.len() {
        let e = &extra[i];
        if overlay_match(&out[p], e) {
            out[p] = pick_richer(&out[p], e);
            i += 1;
            p += 1;
            continue;
        }
        match (p + 1..out.len()).find(|&q| overlay_match(&out[q], e)) {
            Some(found) => p = found,
            None => break,
        }
    }
    out.extend_from_slice(&extra[i..]);
    out
}

fn body_prefix(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// Like [`row_equal`], but a live streaming row can overlay the store's
/// settled copy of the same item (warm remount).
fn row_equal_loose(p: &AgentMsg, l: &AgentMsg) -> bool {
    if l.queued || p.queued {
        return false;
    }
    if p.me && l.me {
        return same_user_chat_body(&p.body, &l.body);
    }
    if has(&p.tool) && has(&l.tool) {
        return p.tool == l.tool && p.tool_input == l.tool_input;
    }
    if p.thinking && l.thinking {
        return body_prefix(&p.body, &l.body);
    }
    if p.compact && l.compact {
        return p.body == l.body && p.compacting == l.compacting;
    }
    if p.fallback && l.fallback {
        return p.body == l.body && p.description == l.description;
    }
    if p.turn_end || l.turn_end {
        return p.turn_end && l.turn_end;
    }
    let loose_plain = |m: &AgentMsg| {
        !m.me && !has(&m.tool) && !m.thinking && !m.compact && !m.fallback && !m.turn_end
    };
    if loose_plain(p) && loose_plain(l) {
        return body_prefix(&p.body, &l.body);
    }
    false
}

fn or_str(a: &str, b: &str) -> String {
    if has(a) { a } else { b }.to_owned()
}

fn or_ts(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    if ts_truthy(a) { a } else { b }
}

/// `{...persisted, ...live}` for two tool rows: live's set fields win.
// simplify: an unset field cannot shadow a set one, where the TS spread lets
// a live key that is explicitly `undefined`/`false` win. Upgrade to
// `Option` fields if a row ever needs to clear a persisted value this way.
fn spread_tool(p: &AgentMsg, l: &AgentMsg) -> AgentMsg {
    let pick = |a: &str, b: &str| or_str(b, a);
    AgentMsg {
        body: pick(&p.body, &l.body),
        tool: pick(&p.tool, &l.tool),
        tool_input: pick(&p.tool_input, &l.tool_input),
        tool_id: pick(&p.tool_id, &l.tool_id),
        description: pick(&p.description, &l.description),
        input: pick(&p.input, &l.input),
        result: pick(&p.result, &l.result),
        result_error: p.result_error || l.result_error,
        result_truncated: p.result_truncated || l.result_truncated,
        provider_uuid: pick(&p.provider_uuid, &l.provider_uuid),
        children: if l.children.is_empty() {
            p.children.clone()
        } else {
            l.children.clone()
        },
        ts: if l.ts.is_some() { l.ts } else { p.ts },
        ..p.clone()
    }
}

fn pick_richer(p: &AgentMsg, l: &AgentMsg) -> AgentMsg {
    if l.streaming {
        let mut out = l.clone();
        if has(&l.tool_id) || has(&p.tool_id) {
            out.tool_id = or_str(&l.tool_id, &p.tool_id);
        }
        if ts_truthy(l.ts) || ts_truthy(p.ts) {
            out.ts = or_ts(l.ts, p.ts);
        }
        return out;
    }
    if p.me && l.me {
        return p.clone();
    }
    if has(&p.tool) && has(&l.tool) {
        let result = or_str(&l.result, &p.result);
        return AgentMsg {
            tool_id: or_str(&l.tool_id, &p.tool_id),
            ts: or_ts(p.ts, l.ts),
            result_pending: !has(&result) && (l.result_pending || p.result_pending),
            result,
            ..spread_tool(p, l)
        };
    }
    if utf16_len(&l.body) > utf16_len(&p.body) {
        let mut out = l.clone();
        if ts_truthy(p.ts) && !ts_truthy(l.ts) {
            out.ts = p.ts;
        }
        return out;
    }
    p.clone()
}

/// Overlays a full in-memory transcript onto a store snapshot. Live may be
/// behind (missing later tools) or parallel (streaming where the store
/// settled).
fn stitch_full_transcripts(persisted: &[AgentMsg], live: &[AgentMsg]) -> Vec<AgentMsg> {
    let mut out = Vec::with_capacity(persisted.len().max(live.len()));
    let (mut i, mut j) = (0, 0);
    while i < persisted.len() && j < live.len() {
        if row_equal_loose(&persisted[i], &live[j]) {
            out.push(pick_richer(&persisted[i], &live[j]));
            i += 1;
            j += 1;
        } else if i + 1 < persisted.len() && row_equal_loose(&persisted[i + 1], &live[j]) {
            out.push(persisted[i].clone());
            i += 1;
        } else if j + 1 < live.len() && row_equal_loose(&persisted[i], &live[j + 1]) {
            out.push(live[j].clone());
            j += 1;
        } else {
            out.push(persisted[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&persisted[i..]);
    out.extend_from_slice(&live[j..]);
    out
}

fn trailing_user(list: &[AgentMsg]) -> Option<&AgentMsg> {
    list.iter().rev().find(|m| m.me && !m.queued)
}

fn index_after_trailing_user(list: &[AgentMsg]) -> usize {
    list.iter()
        .rposition(|m| m.me && !m.queued)
        .map_or(0, |i| i + 1)
}

fn user_has_image_src(m: &AgentMsg) -> bool {
    m.attachments
        .iter()
        .any(|a| has(&a.data_url) || has(&a.url))
}

fn richer_user_row(a: &AgentMsg, b: &AgentMsg) -> AgentMsg {
    let body = if extract_attachment_ids(&b.body).len() > extract_attachment_ids(&a.body).len() {
        b.body.clone()
    } else {
        a.body.clone()
    };
    let attachments = if user_has_image_src(b) && !user_has_image_src(a) {
        b.attachments.clone()
    } else {
        a.attachments.clone()
    };
    AgentMsg {
        body,
        attachments,
        optimistic: false,
        queued: false,
        ..a.clone()
    }
}

fn user_send_ids(m: &AgentMsg) -> Vec<String> {
    let marked = extract_attachment_ids(&m.body);
    if !marked.is_empty() {
        return marked;
    }
    m.attachments
        .iter()
        .filter(|a| has(&a.id))
        .map(|a| a.id.clone())
        .collect()
}

fn is_same_send_replay(prev: &AgentMsg, next: &AgentMsg) -> bool {
    same_user_chat_body(&prev.body, &next.body) && prev.optimistic && !next.optimistic
}

fn persist_has_user_send(mapped: &[AgentMsg], idx: usize, live: &[AgentMsg]) -> bool {
    let live_row = &live[idx];
    let Some(last) = trailing_user(mapped) else {
        return false;
    };
    if !same_user_chat_body(&last.body, &live_row.body) {
        return false;
    }
    let live_ids = user_send_ids(live_row);
    if !live_ids.is_empty() && user_send_ids(last).is_empty() {
        return false;
    }
    if live_ids.is_empty() && !user_send_ids(last).is_empty() {
        let settled = |m: &AgentMsg| m.me && !m.optimistic && !m.queued && m.body == last.body;
        for m in live[..idx].iter().rev() {
            if m.turn_end || is_assistant_text(m) {
                break;
            }
            if settled(m) {
                return false;
            }
        }
        for m in &live[idx + 1..] {
            if m.turn_end || is_assistant_text(m) {
                break;
            }
            if settled(m) {
                return true;
            }
        }
        return last_user_turn_still_open(mapped);
    }
    true
}

fn last_user_turn_still_open(list: &[AgentMsg]) -> bool {
    let after = index_after_trailing_user(list);
    after != 0
        && !list[after..]
            .iter()
            .any(|m| m.turn_end || is_assistant_text(m))
}

fn is_assistant_text(m: &AgentMsg) -> bool {
    if m.me
        || m.queued
        || m.turn_end
        || has(&m.tool)
        || m.thinking
        || m.compact
        || m.note
        || m.fallback
        || m.log.is_some()
        || m.stopped
        || m.error
    {
        return false;
    }
    !js::trim(&m.body).is_empty()
}

/// Drops later user rows that replay the same send within one turn (queued
/// flush + `session.user` + store resync). A resend after assistant text or
/// a footer stays.
pub fn collapse_duplicate_users_in_turn(list: &[AgentMsg]) -> Vec<AgentMsg> {
    let mut out: Vec<AgentMsg> = Vec::with_capacity(list.len());
    for m in list {
        if !m.me || m.queued {
            out.push(m.clone());
            continue;
        }
        let mut saw_assistant = false;
        let mut hit = None;
        for (i, r) in out.iter().enumerate().rev() {
            if r.turn_end {
                break;
            }
            if r.me && !r.queued {
                if !saw_assistant && is_same_send_replay(r, m) {
                    hit = Some(i);
                }
                break;
            }
            if is_assistant_text(r) {
                saw_assistant = true;
            }
        }
        match hit {
            Some(i) => out[i] = richer_user_row(&out[i], m),
            None => out.push(m.clone()),
        }
    }
    out
}

/// Grafts a durable transcript onto a live turn: a stream-only tail aligns
/// onto the store snapshot (pending tools are not duplicated, settled live
/// rows newer than the snapshot stay); a live list that already has the
/// conversation wins.
pub fn merge_persisted_with_live(persisted: &[AgentMsg], live: &[AgentMsg]) -> Vec<AgentMsg> {
    collapse_duplicate_users_in_turn(&merge_live_onto_persist(persisted, live))
}

fn merge_live_onto_persist(persisted: &[AgentMsg], live: &[AgentMsg]) -> Vec<AgentMsg> {
    if live.is_empty() {
        return persisted.to_vec();
    }
    if persisted.is_empty() {
        return live.to_vec();
    }
    let core: Vec<AgentMsg> = live.iter().filter(|m| !m.queued).cloned().collect();
    if core.is_empty() {
        return persisted.to_vec();
    }
    if user_turn_count(&core) >= user_turn_count(persisted) && core.len() >= persisted.len() {
        return core;
    }
    // The same display prompt on a later dispatch must not stitch from
    // persisted[0]: that overlays the live tail onto turn 1.
    if !core[0].optimistic
        && row_equal_loose(&persisted[0], &core[0])
        && user_turn_count(&core) >= user_turn_count(persisted)
    {
        return stitch_full_transcripts(persisted, &core);
    }
    let start = live_align_start(persisted, &core);
    if start < persisted.len() {
        let mut out = persisted[..start].to_vec();
        out.extend(core);
        return out;
    }
    // Live shares a history prefix but is not a suffix overlay: keep the
    // snapshot and append only the live rows after that prefix.
    let prefix = common_prefix_len(persisted, &core);
    if prefix > 0 {
        return overlay_aligned(persisted, &core[prefix..], prefix);
    }
    // Leading live users that replay the last persisted send are the same
    // turn; a lone optimistic user is a resend.
    let mut skip = 0;
    if let Some(last) = trailing_user(persisted) {
        while skip < core.len()
            && core[skip].me
            && !core[skip].queued
            && same_user_chat_body(&core[skip].body, &last.body)
        {
            skip += 1;
        }
    }
    let concat = |a: &[AgentMsg], b: &[AgentMsg]| [a, b].concat();
    if skip == 0 {
        return concat(persisted, &core);
    }
    let extra = &core[skip..];
    let from = index_after_trailing_user(persisted);
    if extra.is_empty() {
        if last_user_turn_still_open(persisted) {
            return persisted.to_vec();
        }
        if skip == 1 && core[0].optimistic {
            return concat(persisted, &core);
        }
        return persisted.to_vec();
    }
    if persisted[from..].iter().any(|m| m.turn_end) && skip == 1 && core[0].optimistic {
        return [persisted, &core[..1], extra].concat();
    }
    let overlaid = overlay_aligned(persisted, extra, from);
    let matched = extra.len() as isize - (overlaid.len() as isize - persisted.len() as isize);
    if last_user_turn_still_open(persisted) || matched > 0 {
        return overlaid;
    }
    let opt: Vec<AgentMsg> = core[..skip]
        .iter()
        .filter(|m| m.optimistic)
        .cloned()
        .collect();
    if opt.is_empty() {
        return overlaid;
    }
    [persisted, &opt, &overlaid[persisted.len()..]].concat()
}

/// Keeps optimistic user bubbles the store snapshot has not recorded yet.
/// Idle overwrites drop live stream rows on purpose; they must not also wipe
/// a send that has not landed.
pub fn retain_optimistic_users(mapped: &[AgentMsg], live: &[AgentMsg]) -> Vec<AgentMsg> {
    let mut out = mapped.to_vec();
    out.extend(
        live.iter()
            .enumerate()
            .filter(|(i, m)| m.optimistic && m.me && !persist_has_user_send(mapped, *i, live))
            .map(|(_, m)| m.clone()),
    );
    collapse_duplicate_users_in_turn(&out)
}

/// A persisted session transcript as chat rows.
pub fn map_chat_items(items: &[ChatItem]) -> Vec<AgentMsg> {
    let mapped: Vec<AgentMsg> = items.iter().filter_map(map_chat_item).collect();
    mark_trailing_widen_stream(collapse_agent_error_dupes(&mapped))
}

/// Store snapshots omit `streaming`: the trailing assistant row is marked so
/// a remount's next cumulative event widens it instead of duplicating.
fn mark_trailing_widen_stream(mut list: Vec<AgentMsg>) -> Vec<AgentMsg> {
    let Some(last) = list.last_mut() else {
        return list;
    };
    if last.me
        || has(&last.tool)
        || last.turn_end
        || last.compact
        || last.note
        || last.fallback
        || last.stopped
        || last.error
        || last.streaming
        || (last.body.is_empty() && !last.thinking)
    {
        return list;
    }
    last.widen_stream = true;
    list
}

fn map_chat_item(it: &ChatItem) -> Option<AgentMsg> {
    let ts = Some(it.ts);
    match it.kind.as_str() {
        "tool" => {
            return Some(AgentMsg {
                tool: it.tool.clone(),
                tool_input: it.tool_input.clone(),
                tool_id: it.tool_id.clone(),
                description: it.description.clone(),
                input: it.input.clone(),
                result: it.result.clone(),
                result_error: it.result_error,
                result_truncated: it.result_truncated,
                result_pending: it.result_pending,
                ts,
                children: it.children.iter().filter_map(map_chat_item).collect(),
                ..AgentMsg::default()
            });
        }
        "thinking" => {
            return Some(AgentMsg {
                thinking: true,
                body: it.text.clone(),
                tokens: it.tokens,
                ts,
                ..AgentMsg::default()
            });
        }
        "compact" => {
            return Some(AgentMsg {
                compact: true,
                body: it.text.clone(),
                tokens: it.tokens,
                ts,
                ..AgentMsg::default()
            });
        }
        "fallback" => {
            return Some(AgentMsg {
                fallback: true,
                body: it.text.clone(),
                description: it.description.clone(),
                ts,
                ..AgentMsg::default()
            });
        }
        "turn" => {
            return Some(AgentMsg {
                turn_end: true,
                duration_ms: it.duration_ms,
                message_id: it.message_id.clone(),
                checkpoint: it.checkpoint,
                ts,
                ..AgentMsg::default()
            });
        }
        _ => {}
    }
    let user = it.role == "user";
    if user && (is_retry_display(&it.text) || is_mode_switch_continue_display(&it.text)) {
        return None;
    }
    if user && let Some(label) = subagent_notice_label(&it.text) {
        return (!label.is_empty()).then(|| AgentMsg {
            note: true,
            note_label: "Subagent".into(),
            body: label,
            ts,
            ..AgentMsg::default()
        });
    }
    if !user && is_chat_stopped_reply(&it.text) {
        return Some(AgentMsg {
            body: CHAT_STOPPED_REPLY.into(),
            stopped: true,
            ts,
            ..AgentMsg::default()
        });
    }
    if !user && is_agent_error_reply(&it.text) {
        return Some(AgentMsg {
            body: it.text.clone(),
            error: true,
            ts,
            ..AgentMsg::default()
        });
    }
    Some(AgentMsg {
        // User text is already display_text || text from Go flatten;
        // display_user_chat_text covers wire that landed without it.
        body: if user {
            display_user_chat_text(&it.text)
        } else {
            it.text.clone()
        },
        me: user,
        ts,
        attachments: it
            .attachments
            .iter()
            .map(|a| ChatAttachmentView {
                id: a.id.clone(),
                name: or_str(&a.name, &a.id),
                mime: a.mime.clone(),
                url: a.url.clone(),
                data_url: String::new(),
            })
            .collect(),
        ..AgentMsg::default()
    })
}

/// A Send*Message attachment: base64 without the `data:` prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub data: String,
}

pub fn to_send_attachments(drafts: &[ChatAttachmentDraft]) -> Vec<SendAttachment> {
    drafts
        .iter()
        .map(|d| SendAttachment {
            id: d.id.clone(),
            name: d.name.clone(),
            mime: d.mime.clone(),
            data: d
                .data_url
                .split_once(',')
                .map_or(d.data_url.clone(), |(_, b)| b.to_owned()),
        })
        .collect()
}

fn base36(mut n: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// A fresh draft id: `img_<base36 ms>_<6 random base36>`.
pub fn new_attachment_id(now_ms: i64) -> String {
    let rand = base36(u128::from_le_bytes(*uuid::Uuid::new_v4().as_bytes()));
    let tail: String = rand.chars().rev().take(6).collect();
    format!("img_{}_{tail}", base36(now_ms.max(0) as u128))
}

fn is_open_stream_kind(m: &AgentMsg, kind: Option<StreamKind>) -> bool {
    m.streaming
        && !m.me
        && !has(&m.tool)
        && !m.compact
        && !m.note
        && !m.fallback
        && kind.is_none_or(|k| m.thinking == (k == StreamKind::Thinking))
}

/// Clears live text/thinking flags in the open turn, so a steer sitting
/// after a still-streaming row cannot open a second live-text row.
fn close_open_streams(out: &mut [AgentMsg], kind: Option<StreamKind>) {
    for m in out.iter_mut().rev() {
        if m.turn_end {
            break;
        }
        if is_open_stream_kind(m, kind) {
            m.streaming = false;
        }
    }
}

fn close_tail(out: &mut [AgentMsg]) {
    if let Some(last) = out.last_mut() {
        last.streaming = false;
    }
}

/// The live stream a delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Text,
    Thinking,
}

fn stream_row(kind: StreamKind, body: &str, streaming: bool, uuid: &str) -> AgentMsg {
    AgentMsg {
        thinking: kind == StreamKind::Thinking,
        body: body.into(),
        streaming,
        provider_uuid: uuid.into(),
        ..AgentMsg::default()
    }
}

/// Extends the trailing streaming row of `kind` by appending a chunk (the
/// legacy non-snapshot path; live deltas use [`apply_stream`]).
pub fn append_stream(list: &[AgentMsg], kind: StreamKind, chunk: &str) -> Vec<AgentMsg> {
    let mut out = list.to_vec();
    if let Some(last) = out.last_mut()
        && last.streaming
        && !has(&last.tool)
        && !last.compact
        && !last.note
        && !last.fallback
        && last.thinking == (kind == StreamKind::Thinking)
    {
        last.body.push_str(chunk);
        return out;
    }
    close_open_streams(&mut out, Some(kind));
    close_tail(&mut out);
    out.push(stream_row(kind, chunk, true, ""));
    out
}

/// Options of one [`apply_stream`] snapshot.
#[derive(Debug, Clone, Default)]
pub struct StreamOpts {
    /// The block finished: close the row so the next tool/reply starts fresh.
    pub done: bool,
    /// The parent Task's tool id for nested subagent output.
    pub parent_id: String,
    /// The provider frame id; a new id on a live row opens a new row.
    pub uuid: String,
}

fn find_tool_path(list: &[AgentMsg], id: &str) -> Option<Vec<usize>> {
    if id.is_empty() {
        return None;
    }
    for (i, m) in list.iter().enumerate().rev() {
        if has(&m.tool) && m.tool_id == id {
            return Some(vec![i]);
        }
        if !m.children.is_empty()
            && let Some(inner) = find_tool_path(&m.children, id)
        {
            return Some([vec![i], inner].concat());
        }
    }
    None
}

fn row_at<'a>(list: &'a [AgentMsg], path: &[usize]) -> Option<&'a AgentMsg> {
    let (first, rest) = path.split_first()?;
    let row = list.get(*first)?;
    if rest.is_empty() {
        Some(row)
    } else {
        row_at(&row.children, rest)
    }
}

fn update_at_path(
    list: &[AgentMsg],
    path: &[usize],
    f: &dyn Fn(&AgentMsg) -> AgentMsg,
) -> Vec<AgentMsg> {
    let mut out = list.to_vec();
    let Some((&i, rest)) = path.split_first() else {
        return out;
    };
    if i >= out.len() {
        return out;
    }
    if rest.is_empty() {
        out[i] = f(&out[i]);
    } else {
        out[i].children = update_at_path(&out[i].children, rest, f);
    }
    out
}

fn is_kind(m: &AgentMsg, kind: StreamKind) -> bool {
    !has(&m.tool)
        && !m.turn_end
        && !m.me
        && !m.compact
        && !m.note
        && !m.fallback
        && m.thinking == (kind == StreamKind::Thinking)
}

/// Stamps a cumulative snapshot onto the trailing streaming row of `kind` (or
/// opens one). The body is replaced, not appended; `done` closes the row.
pub fn apply_stream(
    list: &[AgentMsg],
    kind: StreamKind,
    text: &str,
    opts: &StreamOpts,
) -> Vec<AgentMsg> {
    if has(&opts.parent_id) {
        let Some(path) = find_tool_path(list, &opts.parent_id) else {
            return list.to_vec();
        };
        let inner = StreamOpts {
            done: opts.done,
            uuid: opts.uuid.clone(),
            parent_id: String::new(),
        };
        return update_at_path(list, &path, &|parent| AgentMsg {
            children: apply_stream(&parent.children, kind, text, &inner),
            ..parent.clone()
        });
    }
    if text.is_empty() && !opts.done {
        return list.to_vec();
    }
    let mut out = list.to_vec();
    let streaming = !opts.done;
    let uuid = js::trim(&opts.uuid).to_owned();
    if let Some(last) = out.last_mut()
        && last.streaming
        && is_kind(last, kind)
    {
        if has(&uuid) && has(&last.provider_uuid) && uuid != last.provider_uuid {
            last.streaming = false;
        } else {
            if has(text) {
                last.body = text.to_owned();
            }
            last.streaming = streaming;
            if has(&uuid) {
                last.provider_uuid = uuid;
            }
            if kind == StreamKind::Thinking {
                last.thinking = true;
            }
            return out;
        }
    }
    // Warm remount: widen only a row mapChatItems marked, and only for a
    // continuation; a second consecutive block opens a new row.
    if let Some(last) = out.last_mut()
        && is_kind(last, kind)
        && !opts.done
        && last.widen_stream
    {
        if last.body.is_empty() || text.starts_with(last.body.as_str()) {
            if has(text) {
                last.body = text.to_owned();
            }
            last.streaming = streaming;
            last.widen_stream = false;
            if has(&uuid) {
                last.provider_uuid = uuid;
            }
            if kind == StreamKind::Thinking {
                last.thinking = true;
            }
            return out;
        }
        last.widen_stream = false;
    }
    // Final settle when the live row was already closed (or never opened).
    // Stay in the current tail: walking past a tool would rewrite the first
    // thought with every later snapshot.
    if opts.done {
        for m in out.iter_mut().rev() {
            if m.me || m.turn_end {
                break;
            }
            if is_kind(m, kind) {
                if has(text) {
                    m.body = text.to_owned();
                }
                m.streaming = false;
                return out;
            }
            if has(&m.tool) || m.compact || m.note || m.fallback || m.thinking || has(&m.body) {
                break;
            }
        }
    }
    close_open_streams(&mut out, Some(kind));
    close_tail(&mut out);
    out.push(stream_row(kind, text, streaming, &uuid));
    out
}

/// The provider's complete thinking block, closing the live row.
pub fn settle_thinking(list: &[AgentMsg], text: &str) -> Vec<AgentMsg> {
    apply_stream(
        list,
        StreamKind::Thinking,
        text,
        &StreamOpts {
            done: true,
            ..StreamOpts::default()
        },
    )
}

/// The provider's complete assistant text, closing the live row.
pub fn settle_text(list: &[AgentMsg], text: &str) -> Vec<AgentMsg> {
    apply_stream(
        list,
        StreamKind::Text,
        text,
        &StreamOpts {
            done: true,
            ..StreamOpts::default()
        },
    )
}

/// Redacted-thinking progress: bumps the live thinking row's token count or
/// opens one. Per-event uuids are progress on the same thought.
pub fn note_thinking_tokens(
    list: &[AgentMsg],
    tokens: i64,
    uuid: &str,
    parent_id: &str,
) -> Vec<AgentMsg> {
    if has(parent_id) {
        let Some(path) = find_tool_path(list, parent_id) else {
            return list.to_vec();
        };
        return update_at_path(list, &path, &|parent| AgentMsg {
            children: note_thinking_tokens(&parent.children, tokens, uuid, ""),
            ..parent.clone()
        });
    }
    let mut out = list.to_vec();
    let id = js::trim(uuid);
    if let Some(last) = out.last_mut()
        && last.streaming
        && last.thinking
    {
        last.tokens = last.tokens.max(tokens);
        if !has(&last.provider_uuid) {
            last.provider_uuid = id.to_owned();
        }
        return out;
    }
    close_open_streams(&mut out, Some(StreamKind::Thinking));
    close_tail(&mut out);
    out.push(AgentMsg {
        thinking: true,
        streaming: true,
        tokens,
        provider_uuid: id.to_owned(),
        ..AgentMsg::default()
    });
    out
}

fn settle_in_flight(m: &AgentMsg) -> AgentMsg {
    AgentMsg {
        streaming: false,
        result_pending: false,
        children: m.children.iter().map(settle_in_flight).collect(),
        ..m.clone()
    }
}

/// The label of an engine-to-agent notice (Go `displayHiddenSentinel`): the
/// bare marker is a hidden bubble (`Some("")`), marker + label a "Subagent"
/// row; `None` for ordinary text.
pub fn subagent_notice_label(text: &str) -> Option<String> {
    text.strip_prefix(DISPLAY_HIDDEN_SENTINEL)
        .map(|rest| js::trim(rest).to_owned())
}

/// Appends a mid-turn user message (`session.user`), settling the oldest
/// unmatched optimistic row of this body; a real row of the same text is a
/// new message.
pub fn add_user(list: &[AgentMsg], text: &str, now: i64) -> Vec<AgentMsg> {
    if let Some(label) = subagent_notice_label(text) {
        if label.is_empty() {
            return list.to_vec();
        }
        let mut out = list.to_vec();
        close_open_streams(&mut out, None);
        out.push(AgentMsg {
            note: true,
            note_label: "Subagent".into(),
            body: label,
            ts: Some(now),
            ..AgentMsg::default()
        });
        return out;
    }
    let body = js::trim(text);
    if body.is_empty() {
        return list.to_vec();
    }
    let turn_start = list.iter().rposition(|m| m.turn_end).map_or(0, |i| i + 1);
    let mut out = list.to_vec();
    close_open_streams(&mut out, None);
    if let Some(m) = out[turn_start..]
        .iter_mut()
        .find(|m| m.me && m.optimistic && same_user_chat_body(&m.body, body))
    {
        m.optimistic = false;
        m.body = body.to_owned();
        m.ts = m.ts.or(Some(now));
        return out;
    }
    out.push(AgentMsg {
        me: true,
        body: body.to_owned(),
        ts: Some(now),
        ..AgentMsg::default()
    });
    out
}

fn richer_json(next: &str, prev: &str) -> String {
    let n = if has(next) && next != "{}" { next } else { "" };
    let p = if has(prev) && prev != "{}" { prev } else { "" };
    if n.is_empty() {
        return prev.to_owned();
    }
    if p.is_empty() {
        return next.to_owned();
    }
    if utf16_len(n) >= utf16_len(p) {
        next
    } else {
        prev
    }
    .to_owned()
}

/// Upserts a live tool row. A re-reported call with the same provider id
/// updates its still-open row (a settled match is a real second call); a
/// blank id reuses only the last in-flight row of that tool in this turn.
/// Nested subagent calls land under their parent Task card.
pub fn add_tool(list: &[AgentMsg], step: &WorkspaceStep) -> Vec<AgentMsg> {
    if has(&step.parent_id) {
        let Some(path) = find_tool_path(list, &step.parent_id) else {
            return list.to_vec();
        };
        let child = WorkspaceStep {
            parent_id: String::new(),
            ..step.clone()
        };
        return update_at_path(list, &path, &|parent| AgentMsg {
            children: add_tool(&parent.children, &child),
            ..parent.clone()
        });
    }
    let patch = |m: &AgentMsg| AgentMsg {
        tool: or_str(&step.tool, &m.tool),
        tool_input: or_str(&step.tool_input, &m.tool_input),
        tool_id: or_str(&step.id, &m.tool_id),
        description: or_str(&step.description, &m.description),
        // A later stub must not freeze over a richer body already shown.
        input: richer_json(&step.input, &m.input),
        result_pending: true,
        ..m.clone()
    };
    let adopt = |check_input: bool| {
        for (i, m) in list.iter().enumerate().rev() {
            if (m.me && !m.queued) || m.turn_end {
                break;
            }
            if m.tool == step.tool && m.result_pending && m.tool_id.is_empty() {
                if check_input
                    && has(&step.tool_input)
                    && has(&m.tool_input)
                    && step.tool_input != m.tool_input
                {
                    continue;
                }
                let mut out = list.to_vec();
                out[i] = patch(m);
                return Some(out);
            }
        }
        None
    };
    if has(&step.id) {
        if let Some(path) = find_tool_path(list, &step.id)
            && row_at(list, &path).is_some_and(|r| r.result_pending)
        {
            return update_at_path(list, &path, &patch);
        }
        // mapChatItems omits provider ids: after a remount the next step
        // adopts that row instead of appending a second card.
        if let Some(out) = adopt(true) {
            return out;
        }
    } else if let Some(out) = adopt(false) {
        return out;
    }
    let mut out: Vec<AgentMsg> = list
        .iter()
        .map(|m| AgentMsg {
            streaming: false,
            ..m.clone()
        })
        .collect();
    out.push(AgentMsg {
        tool: step.tool.clone(),
        tool_input: step.tool_input.clone(),
        tool_id: step.id.clone(),
        description: step.description.clone(),
        input: step.input.clone(),
        provider_uuid: step.uuid.clone(),
        // In flight until the result arrives; an empty result is legitimate.
        result_pending: true,
        ..AgentMsg::default()
    });
    out
}

/// The steps list key: tool ids repeat once a settled id gets a second row,
/// so the index is mixed in.
pub fn step_row_key(tool_id: &str, i: usize) -> String {
    if has(tool_id) {
        format!("id:{tool_id}:{i}")
    } else {
        format!("i{i}")
    }
}

/// Args that arrived only with a tool result (Codex MCP).
#[derive(Debug, Clone, Default)]
pub struct ToolExtras {
    pub input: String,
    pub tool_input: String,
}

/// Fills a live tool call's result into its row (newest match by provider
/// id, nested children included) and drops in-flight clones of that id.
pub fn attach_tool_result(
    list: &[AgentMsg],
    id: &str,
    result: &str,
    is_error: bool,
    extras: &ToolExtras,
) -> Vec<AgentMsg> {
    attach_tool_result_in(list, id, result, is_error, extras).unwrap_or_else(|| list.to_vec())
}

fn attach_tool_result_in(
    list: &[AgentMsg],
    id: &str,
    result: &str,
    is_error: bool,
    extras: &ToolExtras,
) -> Option<Vec<AgentMsg>> {
    let Some(hit) = list.iter().rposition(|m| has(&m.tool) && m.tool_id == id) else {
        for (i, m) in list.iter().enumerate() {
            if m.children.is_empty() {
                continue;
            }
            if let Some(nested) = attach_tool_result_in(&m.children, id, result, is_error, extras) {
                let mut out = list.to_vec();
                out[i].children = nested;
                return Some(out);
            }
        }
        return None;
    };
    let cur = &list[hit];
    let settled = AgentMsg {
        result: result.to_owned(),
        result_error: is_error,
        result_pending: false,
        input: if has(&extras.input) && extras.input != "{}" {
            extras.input.clone()
        } else {
            cur.input.clone()
        },
        tool_input: or_str(&extras.tool_input, &cur.tool_input),
        ..cur.clone()
    };
    // Blank ids are not unique, so they are never a drop key.
    Some(
        list.iter()
            .enumerate()
            .filter_map(|(i, m)| {
                if i == hit {
                    Some(settled.clone())
                } else if has(id) && has(&m.tool) && m.tool_id == id && m.result_pending {
                    None
                } else {
                    Some(m.clone())
                }
            })
            .collect(),
    )
}

/// Refreshes a tool row's args only (ask_user park backfill).
pub fn patch_tool_input(list: &[AgentMsg], id: &str, extras: &ToolExtras) -> Vec<AgentMsg> {
    let input = if has(&extras.input) && extras.input != "{}" {
        extras.input.as_str()
    } else {
        ""
    };
    let tool_input = extras.tool_input.as_str();
    if input.is_empty() && tool_input.is_empty() {
        return list.to_vec();
    }
    let Some(path) = find_tool_path(list, id) else {
        return list.to_vec();
    };
    update_at_path(list, &path, &|m| AgentMsg {
        input: or_str(input, &m.input),
        tool_input: or_str(tool_input, &m.tool_input),
        ..m.clone()
    })
}

/// Appends the turn footer with the provider's wall time, settling in-flight
/// rows. A repeat footer leaves the list as is.
pub fn end_turn(list: &[AgentMsg], duration_ms: i64, now: i64) -> Vec<AgentMsg> {
    if duration_ms == 0 || list.last().is_some_and(|m| m.turn_end) {
        return list.to_vec();
    }
    let mut out: Vec<AgentMsg> = list.iter().map(settle_in_flight).collect();
    out.push(AgentMsg {
        turn_end: true,
        duration_ms,
        ts: Some(now),
        ..AgentMsg::default()
    });
    out
}

/// Whether the open turn already ends on its footer. A turn stamped at or
/// after the footer (Retry, autonomous turns) reopens the clock, as does any
/// new user/stream/tool row.
pub fn current_turn_has_ended(list: &[AgentMsg], turn_started_at: i64) -> bool {
    for m in list.iter().rev() {
        if m.queued || m.log.is_some() {
            continue;
        }
        if !m.turn_end {
            return false;
        }
        let footer_at = m.ts.unwrap_or(0);
        return !(turn_started_at > 0 && turn_started_at >= footer_at);
    }
    false
}

/// Whether a live append belongs to a turn whose footer already landed:
/// leftovers that still carry the old stamp drop, as do events after idle
/// while the engine is dark; a busy engine with a cleared stamp is a new
/// turn.
pub fn drop_live_after_footer(list: &[AgentMsg], turn_started_at: i64, engine_busy: bool) -> bool {
    if !current_turn_has_ended(list, turn_started_at) {
        return false;
    }
    turn_started_at > 0 || !engine_busy
}

/// Whether a running status should stamp (or replace) `turnStartedAt`.
pub fn should_refresh_turn_stamp(list: &[AgentMsg], turn_started_at: i64) -> bool {
    turn_started_at <= 0 || current_turn_has_ended(list, turn_started_at)
}

/// Whether an idle that reordered past the next turn's start must not clear
/// a stamp already newer than the footer.
pub fn should_ignore_stale_idle(list: &[AgentMsg], turn_started_at: i64) -> bool {
    if turn_started_at <= 0 {
        return false;
    }
    for m in list.iter().rev() {
        if m.queued || m.log.is_some() {
            continue;
        }
        if !m.turn_end {
            return false;
        }
        let footer_at = m.ts.unwrap_or(0);
        return footer_at > 0 && turn_started_at >= footer_at;
    }
    false
}

/// Skips the stamp-clear/resync only for the first idle after a stamp was
/// replaced over a footer (`stale_idle_armed`); later idles settle.
pub fn should_skip_idle_transcript_reset(
    list: &[AgentMsg],
    turn_started_at: i64,
    stale_idle_armed: bool,
) -> bool {
    if !stale_idle_armed {
        return false;
    }
    if should_ignore_stale_idle(list, turn_started_at) {
        return true;
    }
    if turn_started_at <= 0 {
        return false;
    }
    let mut footer_at = 0;
    let mut rows_after_footer = false;
    for m in list {
        if m.queued || m.log.is_some() {
            continue;
        }
        if m.turn_end {
            footer_at = m.ts.unwrap_or(0);
            rows_after_footer = false;
            continue;
        }
        if footer_at > 0 {
            rows_after_footer = true;
        }
    }
    rows_after_footer && turn_started_at >= footer_at
}

/// The last assistant text bubble of the turn `turn_end_index` closes.
pub fn last_assistant_text_in_turn(list: &[AgentMsg], turn_end_index: usize) -> Option<&AgentMsg> {
    if turn_end_index == 0 || turn_end_index >= list.len() {
        return None;
    }
    for m in list[..turn_end_index].iter().rev() {
        if m.me || m.turn_end {
            break;
        }
        if m.log.is_some()
            || has(&m.tool)
            || m.thinking
            || m.compact
            || m.note
            || m.fallback
            || m.stopped
            || m.error
        {
            continue;
        }
        if !js::trim(&m.body).is_empty() {
            return Some(m);
        }
    }
    None
}

/// A `session.compact` event.
#[derive(Debug, Clone, Default)]
pub struct CompactEvt {
    pub status: String,
    pub summary: String,
    pub pre_tokens: i64,
    pub trigger: String,
}

/// Inserts or settles a context-compaction row.
pub fn apply_compact(list: &[AgentMsg], evt: &CompactEvt) -> Vec<AgentMsg> {
    let row = AgentMsg {
        compact: true,
        compacting: evt.status == "started",
        body: evt.summary.clone(),
        tokens: evt.pre_tokens,
        ..AgentMsg::default()
    };
    let mut out = list.to_vec();
    if let Some(last) = out.last_mut()
        && last.compacting
    {
        let body = if has(&row.body) {
            row.body.clone()
        } else {
            last.body.clone()
        };
        *last = AgentMsg {
            compact: true,
            compacting: row.compacting,
            tokens: row.tokens,
            body,
            ..last.clone()
        };
        return out;
    }
    if let Some(last) = out.last_mut()
        && last.streaming
    {
        last.streaming = false;
    }
    out.push(row);
    out
}

/// A live Recur status (drain, retry): neither assistant text nor persisted.
pub fn apply_notice(list: &[AgentMsg], text: &str) -> Vec<AgentMsg> {
    let t = js::trim(text);
    if t.is_empty() {
        return list.to_vec();
    }
    let mut out = list.to_vec();
    if let Some(last) = out.last_mut() {
        if last.note {
            last.body = t.to_owned();
            return out;
        }
        last.streaming = false;
    }
    out.push(AgentMsg {
        note: true,
        body: t.to_owned(),
        ..AgentMsg::default()
    });
    out
}

/// Drops in-progress compaction rows (an interrupted compact never
/// persisted).
pub fn drop_open_compact(list: &[AgentMsg]) -> Vec<AgentMsg> {
    list.iter().filter(|m| !m.compacting).cloned().collect()
}

/// A `session.fallback` event.
#[derive(Debug, Clone, Default)]
pub struct FallbackEvt {
    pub from: String,
    pub to: String,
    pub from_label: String,
    pub to_label: String,
    pub retracted: Vec<String>,
}

fn open_turn_start(list: &[AgentMsg]) -> usize {
    list.iter()
        .rposition(|m| m.me || m.turn_end)
        .map_or(0, |i| i + 1)
}

/// Inserts a model-fallback notice, once per serving model per turn. An
/// incomplete first hop (no from) is enriched by a later audit; retracted
/// frame ids drop the refused leg's live rows.
pub fn apply_fallback(list: &[AgentMsg], evt: &FallbackEvt) -> Vec<AgentMsg> {
    let mut out = if evt.retracted.is_empty() {
        list.to_vec()
    } else {
        drop_retracted_frames(list, &evt.retracted)
    };
    let to = js::trim(if has(&evt.to_label) {
        &evt.to_label
    } else {
        &evt.to
    })
    .to_owned();
    if to.is_empty() {
        return out;
    }
    let from = js::trim(if has(&evt.from_label) {
        &evt.from_label
    } else {
        &evt.from
    })
    .to_owned();
    for i in (0..out.len()).rev() {
        let m = &out[i];
        if m.me || m.turn_end {
            break;
        }
        if m.fallback && m.body == to {
            let have = js::trim(&m.description);
            if has(&from) && have.is_empty() {
                out[i].description = from;
            }
            return out;
        }
    }
    close_tail_if_streaming(&mut out);
    let row = AgentMsg {
        fallback: true,
        body: to,
        description: from,
        ..AgentMsg::default()
    };
    if !evt.retracted.is_empty() {
        let start = open_turn_start(&out);
        out.insert(start, row);
        return out;
    }
    out.push(row);
    out
}

fn close_tail_if_streaming(out: &mut [AgentMsg]) {
    if let Some(last) = out.last_mut()
        && last.streaming
    {
        last.streaming = false;
    }
}

fn drop_retracted_frames(list: &[AgentMsg], uuids: &[String]) -> Vec<AgentMsg> {
    let drop: HashSet<&str> = uuids
        .iter()
        .map(|u| js::trim(u))
        .filter(|u| has(u))
        .collect();
    if drop.is_empty() {
        return list.to_vec();
    }
    let start = open_turn_start(list);
    let tail = &list[start..];
    let matched = tail
        .iter()
        .any(|m| has(&m.provider_uuid) && drop.contains(m.provider_uuid.as_str()));
    let fb = tail.iter().position(|m| m.fallback);
    let mut out = list[..start].to_vec();
    for (i, m) in tail.iter().enumerate() {
        if has(&m.provider_uuid) && drop.contains(m.provider_uuid.as_str()) {
            continue;
        }
        if !matched
            && !has(&m.provider_uuid)
            && !m.fallback
            && fb.is_none_or(|f| i < f)
            && (has(&m.tool) || m.thinking || has(&m.body))
        {
            continue;
        }
        out.push(m.clone());
    }
    out
}

/// Drops an optimistic user row (by pending id, else by display text), even
/// past a footer that already landed.
pub fn drop_optimistic_user(list: &[AgentMsg], display: &str, pending_id: &str) -> Vec<AgentMsg> {
    let hit = list.iter().rposition(|m| {
        m.optimistic
            && m.me
            && if has(pending_id) {
                m.pending_id == pending_id
            } else {
                same_user_chat_body(&m.body, display)
            }
    });
    let mut out = list.to_vec();
    if let Some(i) = hit {
        out.remove(i);
    }
    out
}

/// Rejected steers in send order, then the text typed after them.
// simplify: in-paragraph edits of a restored steer between rejections can
// duplicate. Upgrade: the caller keeps the restored prefix as one unit.
pub fn merge_rejected_steer_draft(failed_marked: &[String], current_plain: &str) -> String {
    let steers: Vec<&str> = failed_marked
        .iter()
        .map(|s| js::trim(s))
        .filter(|s| has(s))
        .collect();
    let mut tail = js::trim(current_plain).to_owned();
    for steer in &steers {
        if tail.is_empty() {
            break;
        }
        if tail == *steer {
            tail.clear();
            continue;
        }
        if let Some(rest) = tail.strip_prefix(&format!("{steer}\n\n")) {
            tail = js::trim(rest).to_owned();
        }
    }
    let mut parts = steers;
    if has(&tail) {
        parts.push(&tail);
    }
    parts.join("\n\n")
}

/// Failed drafts' attachments, then later picks, without duplicate ids.
pub fn merge_rejected_steer_attachments<T: Clone>(
    failed_drafts: &[Vec<T>],
    current: &[T],
    id: impl Fn(&T) -> &str,
) -> Vec<T> {
    let mut seen = HashSet::new();
    failed_drafts
        .iter()
        .flatten()
        .chain(current)
        .filter(|a| has(id(a)) && seen.insert(id(a).to_owned()))
        .cloned()
        .collect()
}

fn is_error_like(m: &AgentMsg) -> bool {
    m.error
        || (has(&m.body)
            && !m.me
            && !has(&m.tool)
            && !m.thinking
            && !m.turn_end
            && !m.stopped
            && is_agent_error_reply(&m.body))
}

/// Folds a provider-echoed error line into the synthesized "Agent error"
/// note and drops a duplicate error row, keeping a footer between them.
pub fn collapse_agent_error_dupes(list: &[AgentMsg]) -> Vec<AgentMsg> {
    let mut out: Vec<AgentMsg> = Vec::with_capacity(list.len());
    for m in list {
        if !is_error_like(m) {
            out.push(m.clone());
            continue;
        }
        // ts stays: identical errors in one chat would collide at 0 as keys.
        let err = AgentMsg {
            body: m.body.clone(),
            error: true,
            ts: Some(m.ts.unwrap_or(0)),
            ..AgentMsg::default()
        };
        let te = if out.last().is_some_and(|l| l.turn_end) {
            out.pop()
        } else {
            None
        };
        if let Some(last) = out.last() {
            if last.error && same_error_detail(&last.body, &err.body) {
                out.extend(te);
                continue;
            }
            if !last.me
                && !has(&last.tool)
                && !last.thinking
                && !last.turn_end
                && !last.error
                && has(&last.body)
                && same_error_detail(&last.body, &err.body)
            {
                out.pop();
            }
        }
        out.push(err);
        out.extend(te);
    }
    out
}

/// Whether the fallback reply stays visible even after streamed text.
pub fn should_keep_fallback_reply(text: &str) -> bool {
    is_agent_error_reply(text) || is_chat_stopped_reply(text)
}

/// Claude Code Task/Agent, Cursor task.
pub fn is_task_tool(tool: &str) -> bool {
    tool == "Task" || tool == "Agent" || tool == "task"
}

/// Claude Code's background-Task launch ack (instructions for the model, not
/// the user). Prefix-matched like the engine's `isSubagentLaunchMeta`.
pub fn is_subagent_launch_meta(text: &str) -> bool {
    js::trim(text).starts_with("Async agent launched successfully")
}

/// Stops every in-flight row; a turn with no assistant text gets the reply
/// (or error/stop note) so it never ends silent. Error and stop notes land
/// even after streamed text. `duration_ms` stamps the footer for user-stop,
/// where the provider often never emits `turn_end`.
pub fn finish_turn(
    list: &[AgentMsg],
    fallback: &str,
    turn_got_text: bool,
    duration_ms: i64,
    now: i64,
) -> Vec<AgentMsg> {
    let closed: Vec<AgentMsg> = list.iter().map(settle_in_flight).collect();
    let fb = js::trim(fallback);
    if fb.is_empty() || (turn_got_text && !should_keep_fallback_reply(fb)) {
        return closed;
    }
    if let Some(last) = closed.last()
        && !last.me
        && !has(&last.tool)
        && !last.thinking
        && !last.turn_end
        && last.body == fb
    {
        if is_chat_stopped_reply(fb) {
            return end_turn(&closed, duration_ms, now);
        }
        if is_agent_error_reply(fb) {
            let mut marked = closed.clone();
            if let Some(l) = marked.last_mut() {
                l.error = true;
            }
            return end_turn(&collapse_agent_error_dupes(&marked), duration_ms, now);
        }
        return closed;
    }
    let mut out = closed;
    if is_chat_stopped_reply(fb) {
        out.push(AgentMsg {
            body: CHAT_STOPPED_REPLY.into(),
            stopped: true,
            ..AgentMsg::default()
        });
        return end_turn(&out, duration_ms, now);
    }
    if is_agent_error_reply(fb) {
        out.push(AgentMsg {
            body: fb.to_owned(),
            error: true,
            ts: Some(now),
            ..AgentMsg::default()
        });
        return end_turn(&collapse_agent_error_dupes(&out), duration_ms, now);
    }
    out.push(AgentMsg {
        body: fb.to_owned(),
        ..AgentMsg::default()
    });
    out
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
