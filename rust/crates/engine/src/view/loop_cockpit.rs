//! The Loop cockpit transcript (frontend `lib/loop-cockpit.ts`): conductor
//! turns merged with specialist sessions (one card per dispatch) and engine
//! facts, plus the PR-opened / wrap fact parsing the cockpit renders. A PA*
//! tab that has not engaged a run keeps its own unmerged thread.

use std::sync::LazyLock;

use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW, FeedItem,
    PrSource,
};
use regex::Regex;

use super::chat::{AgentMsg, ChatLog, ChatLogSession};
use super::js;

/// One specialist session feeding the cockpit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedSess {
    pub id: String,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub ts: i64,
    /// Wall time this live dispatch started; the last slice stamps here,
    /// not at the session's creation.
    pub live_ts: Option<i64>,
    pub messages: Vec<AgentMsg>,
}

/// `PA` or `PA<digits>` (the frontend's looser test than `codes`).
pub fn is_pair_chat_code(code: &str) -> bool {
    code == "PA"
        || code
            .strip_prefix("PA")
            .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
}

pub fn is_specialist_session(code: &str) -> bool {
    code != "MA" && !is_pair_chat_code(code)
}

/// The cockpit belongs on the conductor tab: legacy MA, or a PA* tab whose
/// run is engaged (loop pickers pinned).
pub fn is_loop_cockpit_tab(code: &str, loop_pinned: bool) -> bool {
    code == "MA" || loop_pinned
}

pub fn specialist_name(code: &str) -> String {
    match code {
        "CO" => "Coder",
        "RV" => "Reviewer",
        "TE" => "Tester",
        "QA" => "QA",
        "PL" => "Planner",
        "SR" => "Security reviewer",
        "WR" => "Writer",
        "ED" => "Editor",
        other => other,
    }
    .to_owned()
}

/// The chat-mode slot a specialist runs in (Loop bundle `[[role]].mode`).
pub fn specialist_mode(code: &str) -> &'static str {
    match code {
        "PL" => CHAT_MODE_PLAN,
        "RV" | "SR" | "ED" => CHAT_MODE_REVIEW,
        "QA" => CHAT_MODE_ASK,
        "MA" => CHAT_MODE_LOOP,
        _ => CHAT_MODE_AGENT,
    }
}

/// Specialist codes running on this workspace (agent ids `<ws>-<code>`),
/// after `extra`, first occurrence order.
pub fn specialist_live_codes<'a>(
    workspace_id: &str,
    running: impl IntoIterator<Item = &'a str>,
    extra: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |c: &str| {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_owned());
        }
    };
    for c in extra {
        add(c);
    }
    if workspace_id.is_empty() {
        return out;
    }
    let prefix = format!("{workspace_id}-");
    for id in running {
        if let Some(code) = id.strip_prefix(&prefix)
            && !code.is_empty()
            && is_specialist_session(code)
        {
            add(code);
        }
    }
    out
}

/// The finished-loop label from milliseconds; empty for a non-positive
/// span, where the engine's feed label says `0s`.
pub fn format_loop_elapsed_ms(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let sec = (ms + 500) / 1000;
    if sec < 60 {
        return format!("{sec}s");
    }
    let (m, s) = (sec / 60, sec % 60);
    if m >= 60 {
        let (h, rm) = (m / 60, m % 60);
        return if rm == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {rm}m")
        };
    }
    if s == 0 {
        format!("{m}m")
    } else {
        format!("{m}m {s:02}s")
    }
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());

fn fact_plain(text: &str) -> String {
    js::trim(&TAG.replace_all(text, "")).to_owned()
}

static KICKOFF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(Reviewing the diff \(iteration \d+\)\.|Driving the app in a browser to verify the change\.|Checking the plan against the implementer's handoff\.|Planning the change and definition-of-done\.|Dispatched from chat:)").unwrap()
});
static WRAP_FACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(Committed |Handoff:|No file changes to commit this iteration\.)").unwrap()
});

/// Phase-start chatter the specialist card already covers.
pub fn is_phase_kickoff_fact(text: &str) -> bool {
    KICKOFF.is_match(&fact_plain(text))
}

/// Commit / handoff lines that belong on the specialist card.
pub fn is_specialist_wrap_fact(text: &str) -> bool {
    WRAP_FACT.is_match(&fact_plain(text))
}

fn wrap_fact_message(text: &str, ts: i64) -> AgentMsg {
    let plain = fact_plain(text);
    let label = if plain.starts_with("Handoff:") {
        "Handoff"
    } else if plain.starts_with("Committed ") || plain.starts_with("No file changes") {
        "Commit"
    } else {
        "Notice"
    };
    AgentMsg {
        note: true,
        note_label: label.into(),
        note_html: true,
        body: text.to_owned(),
        ts: Some(ts),
        ..AgentMsg::default()
    }
}

fn ts_or_zero(m: &AgentMsg) -> i64 {
    m.ts.unwrap_or(0)
}

fn insert_by_ts(messages: &mut Vec<AgentMsg>, msg: AgentMsg) {
    let ts = ts_or_zero(&msg);
    let mut i = messages.len();
    while i > 0 && ts_or_zero(&messages[i - 1]) > ts {
        i -= 1;
    }
    messages.insert(i, msg);
}

/// A PR-opened engine fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPrFact {
    pub number: i64,
    pub pr_id: Option<String>,
    pub duration: Option<String>,
    pub source: PrSource,
}

static DATA_PR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"data-pr="([^"]+)""#).unwrap());
static NOT_OPENED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)not opened").unwrap());
static OPENED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Opened|Completed in").unwrap());
static PR_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)PR(?-u:\b)").unwrap());
static TAG_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static PR_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#(\d+)").unwrap());
static COMPLETED_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Completed in ([^·<]+)").unwrap());
static GITHUB_PR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)GitHub PR").unwrap());
static LOCAL_PR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Local PR").unwrap());

/// A PR-opened engine fact: the legacy long sentence, the Completed-in
/// line, or a `loop-pr-link` / `data-pr` anchor. A "not opened" fallback
/// note is not one.
pub fn parse_loop_pr_fact(html: &str) -> Option<LoopPrFact> {
    if html.is_empty() {
        return None;
    }
    let html = strip_loop_wrap(html);
    if NOT_OPENED.is_match(&html) {
        return None;
    }
    let pr_id = DATA_PR.captures(&html).map(|c| c[1].to_owned());
    let is_link = pr_id.is_some() || html.contains("loop-pr-link");
    if !is_link && !OPENED.is_match(&html) {
        return None;
    }
    if !is_link && !PR_WORD.is_match(&TAG_SPACE.replace_all(&html, " ")) {
        return None;
    }
    let number = PR_NUMBER
        .captures(&html)
        .and_then(|c| c[1].parse::<i64>().ok())
        .unwrap_or(0);
    if number == 0 && pr_id.is_none() {
        return None;
    }
    let duration = COMPLETED_IN
        .captures(&html)
        .map(|c| js::trim(&c[1]).to_owned())
        .filter(|d| !d.is_empty());
    let source = if GITHUB_PR.is_match(&html) && !LOCAL_PR.is_match(&html) {
        PrSource::GitHub
    } else {
        PrSource::Local
    };
    Some(LoopPrFact {
        number,
        pr_id,
        duration,
        source,
    })
}

/// Display fields for a PR-opened fact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopPrCompletion {
    pub duration: String,
    pub pr_id: Option<String>,
    pub pr_label: String,
}

/// A workspace PR the cockpit can resolve a legacy fact against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrRef {
    pub id: String,
    pub number: i64,
    pub workspace_id: String,
}

/// What the cockpit knows beyond the fact text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrCompletionCtx {
    pub prs: Vec<PrRef>,
    pub workspace_id: String,
    pub fact_ts: i64,
    pub start_ts: Option<i64>,
}

const THIRTY_DAYS_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// A PR-opened fact's display fields, filling the id from the workspace's
/// PRs and the duration from the cockpit span when the fact lacks them.
/// `start_ts` 1 is the opening-prompt sentinel, never a loop start.
pub fn loop_pr_completion(html: &str, ctx: Option<&PrCompletionCtx>) -> Option<LoopPrCompletion> {
    let parsed = parse_loop_pr_fact(html)?;
    let mut pr_id = parsed.pr_id.clone();
    if pr_id.is_none()
        && parsed.number != 0
        && let Some(c) = ctx
    {
        let ws = c.workspace_id.as_str();
        pr_id = c
            .prs
            .iter()
            .find(|p| p.number == parsed.number && (ws.is_empty() || p.workspace_id == ws))
            .map(|p| p.id.clone());
    }
    let span = match ctx {
        Some(c) => match c.start_ts {
            Some(start) if c.fact_ts != 0 && start > 1 && c.fact_ts > start => c.fact_ts - start,
            _ => 0,
        },
        None => 0,
    };
    let duration = parsed.duration.clone().unwrap_or_else(|| {
        if span > 0 && span < THIRTY_DAYS_MS {
            format_loop_elapsed_ms(span)
        } else {
            String::new()
        }
    });
    let kind = if parsed.source == PrSource::GitHub {
        "GitHub PR"
    } else {
        "Local PR"
    };
    let pr_label = if parsed.number != 0 {
        format!("{kind} #{}", parsed.number)
    } else {
        kind.to_owned()
    };
    Some(LoopPrCompletion {
        duration,
        pr_id,
        pr_label,
    })
}

static LOOP_WRAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<div class="loop-wrap">(.*?)</div>\s*$"#).unwrap());

/// The inner HTML of the engine's loop-wrap body.
pub fn loop_completion_wrap(html: &str) -> String {
    LOOP_WRAP
        .captures(html)
        .map(|c| js::trim(&c[1]).to_owned())
        .unwrap_or_default()
}

/// The fact text without its loop-wrap body.
pub fn strip_loop_wrap(html: &str) -> String {
    js::trim(&LOOP_WRAP.replacen(html, 1, "")).to_owned()
}

/// `Number.MAX_SAFE_INTEGER`: unstamped live rows sort at the tail.
const LIVE_TS: i64 = (1 << 53) - 1;

fn msg_ts(m: &AgentMsg) -> i64 {
    match m.ts {
        Some(t) if t > 0 => t,
        _ if m.optimistic || m.streaming || m.queued => LIVE_TS,
        _ => 0,
    }
}

fn first_ts(messages: &[AgentMsg], fallback: i64) -> i64 {
    messages
        .iter()
        .find_map(|m| m.ts.filter(|t| *t > 0))
        .unwrap_or(fallback)
}

/// The previous dispatch settled: the next patch is a new cockpit card.
pub fn needs_live_turn_break(msgs: &[AgentMsg]) -> bool {
    msgs.last()
        .is_some_and(|l| !(l.turn_end || l.streaming || l.result_pending || l.me))
}

/// One card per dispatch: a session splits on each new user prompt and
/// after each turn end, stamped at that turn's time. Ids always carry the
/// turn index so the first card's key stays stable when another lands.
pub fn split_session_turns(s: &FeedSess) -> Vec<FeedSess> {
    let mut turns: Vec<Vec<AgentMsg>> = Vec::new();
    let mut cur: Vec<AgentMsg> = Vec::new();
    for m in &s.messages {
        if m.me && !cur.is_empty() {
            turns.push(std::mem::take(&mut cur));
        }
        cur.push(m.clone());
        if m.turn_end {
            turns.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        turns.push(cur);
    }
    if turns.is_empty() {
        turns.push(Vec::new());
    }
    let n = turns.len();
    turns
        .into_iter()
        .enumerate()
        .map(|(i, messages)| {
            let last = i == n - 1;
            let fallback = match s.live_ts {
                Some(t) if last && t != 0 => t,
                _ => s.ts,
            };
            // An empty kickoff card stays at the live tail until its
            // prompt stamps.
            let ts = if last && messages.is_empty() {
                LIVE_TS
            } else {
                first_ts(&messages, fallback)
            };
            FeedSess {
                id: format!("{}#{i}", s.id),
                ts,
                messages,
                ..s.clone()
            }
        })
        .collect()
}

fn merge_ts(m: &AgentMsg) -> i64 {
    let t = msg_ts(m);
    if t > 0 { t } else { LIVE_TS }
}

struct Extra {
    ts: i64,
    seq: usize,
    msg: AgentMsg,
}

fn log_row(ts: i64, log: ChatLog) -> AgentMsg {
    AgentMsg {
        ts: Some(ts),
        log: Some(Box::new(log)),
        ..AgentMsg::default()
    }
}

fn session_log(m: &AgentMsg) -> Option<&ChatLogSession> {
    match m.log.as_deref() {
        Some(ChatLog::Session(s)) => Some(s),
        _ => None,
    }
}

/// Interleaves specialist cards and engine facts into the conductor
/// transcript. Conductor rows keep their own order so a live turn cannot
/// lose steps; `opening_prompt` (the loop goal) is pinned as the first user
/// bubble while the conductor only holds optimistic rows; queued follow-ups
/// stay last.
pub fn merge_loop_cockpit(
    ma: &[AgentMsg],
    sessions: &[FeedSess],
    facts: &[FeedItem],
    live_codes: &[String],
    opening_prompt: &str,
) -> Vec<AgentMsg> {
    let (queued, mut ma_rows): (Vec<AgentMsg>, Vec<AgentMsg>) =
        ma.iter().cloned().partition(|m| m.queued);
    let prompt = js::trim(opening_prompt);
    let first_user = ma_rows.iter().find(|m| m.me);
    if !prompt.is_empty() && first_user.is_none_or(|u| u.optimistic && u.body != prompt) {
        ma_rows.insert(
            0,
            AgentMsg {
                me: true,
                body: prompt.to_owned(),
                ts: Some(1),
                ..AgentMsg::default()
            },
        );
    }

    let is_live = |code: &str| live_codes.iter().any(|c| c == code);
    let mut slices: Vec<FeedSess> = sessions
        .iter()
        .filter(|s| is_specialist_session(&s.code))
        .flat_map(split_session_turns)
        .collect();
    let mut have: Vec<String> = slices.iter().map(|s| s.code.clone()).collect();
    for code in live_codes {
        if code.is_empty() || have.contains(code) || !is_specialist_session(code) {
            continue;
        }
        have.push(code.clone());
        slices.extend(split_session_turns(&FeedSess {
            id: format!("live-{code}"),
            code: code.clone(),
            name: specialist_name(code),
            ts: LIVE_TS,
            ..FeedSess::default()
        }));
    }
    let mut live_slice: Vec<(String, String)> = Vec::new();
    for s in &slices {
        if is_live(&s.code) {
            match live_slice.iter_mut().find(|(c, _)| *c == s.code) {
                Some(e) => e.1 = s.id.clone(),
                None => live_slice.push((s.code.clone(), s.id.clone())),
            }
        }
    }
    let mut extras: Vec<Extra> = Vec::new();
    for s in slices {
        let live = live_slice.iter().any(|(c, id)| *c == s.code && *id == s.id);
        extras.push(Extra {
            ts: s.ts,
            seq: extras.len(),
            msg: log_row(
                s.ts,
                ChatLog::Session(ChatLogSession {
                    id: s.id,
                    code: s.code,
                    name: s.name,
                    messages: s.messages,
                    live,
                }),
            ),
        });
    }
    for f in facts {
        if is_phase_kickoff_fact(&f.text) {
            continue;
        }
        let ts = f.ts;
        let code = if f.agent_code.is_empty() {
            "MA"
        } else {
            f.agent_code.as_str()
        };
        if is_specialist_wrap_fact(&f.text) && is_specialist_session(code) {
            let (mut host, mut last) = (None, None);
            for (i, e) in extras.iter().enumerate() {
                if session_log(&e.msg).is_none_or(|s| s.code != code) {
                    continue;
                }
                last = Some(i);
                if e.ts <= ts {
                    host = Some(i);
                }
            }
            if let Some(i) = host.or(last)
                && let Some(ChatLog::Session(s)) = extras[i].msg.log.as_deref_mut()
            {
                insert_by_ts(&mut s.messages, wrap_fact_message(&f.text, ts));
                continue;
            }
        }
        let has_card = !f.card.is_empty();
        extras.push(Extra {
            ts,
            seq: extras.len(),
            msg: log_row(
                ts,
                ChatLog::Fact {
                    id: f.id.clone(),
                    text: f.text.clone(),
                    is_deviation: f.is_deviation,
                    code: code.to_owned(),
                    name: if f.name.is_empty() {
                        "Maestro".into()
                    } else {
                        f.name.clone()
                    },
                    card: f.card.clone(),
                    data: if has_card {
                        f.data.clone()
                    } else {
                        String::new()
                    },
                },
            ),
        });
    }
    extras.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.seq.cmp(&b.seq)));

    let first_user_idx = ma_rows.iter().position(|m| m.me);
    let mut out: Vec<AgentMsg> = Vec::new();
    let mut extras = extras.into_iter().peekable();
    for (i, row) in ma_rows.into_iter().enumerate() {
        // The opening prompt comes first: "Starting loop for…" is stamped
        // before the planner records the goal. Unstamped or streaming rows
        // count as now, so facts land before the live tail.
        if Some(i) != first_user_idx {
            let ts = merge_ts(&row);
            if ts > 0 {
                while let Some(e) = extras.next_if(|e| e.ts > 0 && e.ts < ts) {
                    out.push(e.msg);
                }
            }
        }
        out.push(row);
    }
    out.extend(extras.map(|e| e.msg));
    let mut merged = cluster_sibling_sessions(out);
    merged.extend(queued);
    merged
}

/// Packs adjacent specialist cards into one fanout group.
fn cluster_sibling_sessions(msgs: Vec<AgentMsg>) -> Vec<AgentMsg> {
    let mut out: Vec<AgentMsg> = Vec::new();
    let mut group: Vec<ChatLogSession> = Vec::new();
    let mut group_ts = 0;
    let flush =
        |group: &mut Vec<ChatLogSession>, group_ts: i64, out: &mut Vec<AgentMsg>| match group.len()
        {
            0 => {}
            1 => out.push(log_row(group_ts, ChatLog::Session(group.remove(0)))),
            _ => {
                let sessions = std::mem::take(group);
                out.push(log_row(
                    group_ts,
                    ChatLog::Sessions {
                        id: sessions[0].id.clone(),
                        sessions,
                    },
                ));
            }
        };
    for m in msgs {
        if let Some(s) = session_log(&m) {
            if group.is_empty() {
                group_ts = ts_or_zero(&m);
            }
            group.push(s.clone());
        } else {
            flush(&mut group, group_ts, &mut out);
            out.push(m);
        }
    }
    flush(&mut group, group_ts, &mut out);
    out
}

#[cfg(test)]
#[path = "loop_cockpit_tests.rs"]
mod tests;
