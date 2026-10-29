//! `@terminal[:id-or-title]` mentions: an ANSI-stripped tail of the matching
//! Observatory terminals, fenced into a system instruction (R16: read through
//! [`TerminalSource`]).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::terminals::{TerminalSession, TerminalSource};

/// Keep in sync with the frontend `TERMINAL_AT_RE`.
static TERMINAL_AT_RE: LazyLock<Regex> = LazyLock::new(|| {
    recur_bundles::go::compile_regexp(r"@terminal(?::([^\s@]+))?").expect("terminal pattern")
});

const TERMINAL_WRAP_TAIL_LINES: i64 = recur_term::DEFAULT_TAIL_LINES as i64;
const MAX_TERMINAL_SNAPSHOTS: usize = 4;

/// One `@terminal` chip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalMention {
    /// The token after `@` (`terminal` or `terminal:<qual>`).
    pub chip: String,
    /// Empty for bare `@terminal`.
    pub qual: String,
    pub start: usize,
}

/// A body fenced with enough backticks that it cannot close its own fence,
/// with any `</system_instruction>` defused.
pub fn fence_snapshot(body: &str) -> String {
    let body = scrub_instruction_text(body);
    let mut n = 3;
    while body.contains(&"`".repeat(n)) && n < 32 {
        n += 1;
    }
    let fence = "`".repeat(n);
    let mut b = format!("{fence}\n{body}");
    if !body.ends_with('\n') {
        b.push('\n');
    }
    b.push_str(&fence);
    b.push('\n');
    b
}

/// Defuses an instruction close tag inside quoted content.
pub fn scrub_instruction_text(s: &str) -> String {
    s.replace("</system_instruction>", "< /system_instruction>")
}

pub(crate) fn ws_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Whether `start..end` is a whitespace-bounded token.
pub(crate) fn at_token_bounded(text: &str, start: usize, end: usize) -> bool {
    let b = text.as_bytes();
    if start > 0 && !ws_byte(b[start - 1]) {
        return false;
    }
    end >= b.len() || ws_byte(b[end])
}

/// Unique `@terminal` chips in first-seen order.
pub fn collect_terminal_mentions(text: &str) -> Vec<TerminalMention> {
    if text.is_empty() || !text.contains("@terminal") {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in TERMINAL_AT_RE.captures_iter(text) {
        let Some(m) = c.get(0) else { continue };
        if !at_token_bounded(text, m.start(), m.end()) {
            continue;
        }
        let qual = c.get(1).map_or("", |q| q.as_str()).to_owned();
        let chip = if qual.is_empty() {
            "terminal".to_owned()
        } else {
            format!("terminal:{qual}")
        };
        if !seen.insert(chip.clone()) {
            continue;
        }
        out.push(TerminalMention {
            chip,
            qual,
            start: m.start(),
        });
    }
    out
}

/// [`attach_terminal_mentions`] over the text's own mentions.
pub fn wrap_attached_terminal_mentions(
    text: &str,
    workspace_id: &str,
    src: Option<&dyn TerminalSource>,
) -> String {
    attach_terminal_mentions(text, &collect_terminal_mentions(text), workspace_id, src)
}

/// Prepends a snapshot of each mentioned terminal (at most four).
/// Unchanged with no mentions or no terminal source.
pub fn attach_terminal_mentions(
    text: &str,
    mentions: &[TerminalMention],
    workspace_id: &str,
    src: Option<&dyn TerminalSource>,
) -> String {
    let Some(src) = src else {
        return text.to_owned();
    };
    if mentions.is_empty() || workspace_id.is_empty() {
        return text.to_owned();
    }
    let sessions = src.list(workspace_id);
    let active = src.active_id(workspace_id);
    let mut b = String::from("<system_instruction>\n");
    b.push_str("The user attached Observatory terminal output. It is a snapshot of the live PTY ");
    b.push_str("(ANSI stripped, last lines) — not a worktree file. Do not search the repo for these paths. ");
    b.push_str("For a later snapshot or another tab, call mcp__recur__list_terminals / mcp__recur__read_terminal.\n");
    let mut wrote = 0;
    for m in mentions {
        if wrote >= MAX_TERMINAL_SNAPSHOTS {
            break;
        }
        let hits = resolve_terminal_mentions(m, &sessions, &active);
        if hits.is_empty() {
            b.push_str(&format!("\n### @{}\n(no matching terminal)\n", m.chip));
            wrote += 1;
            continue;
        }
        for info in hits {
            if wrote >= MAX_TERMINAL_SNAPSHOTS {
                break;
            }
            let body = src.read_plain(&info.id, TERMINAL_WRAP_TAIL_LINES);
            let title = match info.title.trim() {
                "" => "terminal",
                t => t,
            };
            b.push_str(&format!(
                "\n### @{} ({title}, id {}{})\n",
                m.chip,
                info.id,
                terminal_exited_note(&info)
            ));
            match body {
                Err(e) => b.push_str(&format!("(could not read: {e})\n")),
                Ok(body) if body.trim().is_empty() => b.push_str("(idle — no recent output)\n"),
                Ok(body) => b.push_str(&fence_snapshot(&body)),
            }
            wrote += 1;
        }
    }
    if wrote == 0 {
        return text.to_owned();
    }
    b.push_str("</system_instruction>\n\n");
    b.push_str(text);
    b
}

/// The sessions a mention names: bare → the focused tab, else the newest
/// live one, else the newest; a qualifier → exact id, then a unique id
/// prefix, then a unique title.
pub fn resolve_terminal_mentions(
    m: &TerminalMention,
    sessions: &[TerminalSession],
    active_id: &str,
) -> Vec<TerminalSession> {
    if sessions.is_empty() {
        return Vec::new();
    }
    if m.qual.is_empty() {
        if !active_id.is_empty()
            && let Some(s) = sessions.iter().find(|s| s.id == active_id)
        {
            return vec![s.clone()];
        }
        return newest_live_or_last(sessions);
    }
    let qual = &m.qual;
    let mut prefix = Vec::new();
    let mut title = Vec::new();
    for s in sessions {
        if &s.id == qual {
            return vec![s.clone()];
        }
        if s.id.starts_with(qual.as_str()) {
            prefix.push(s.clone());
        }
        if s.title.to_lowercase() == qual.to_lowercase() {
            title.push(s.clone());
        }
    }
    if prefix.len() == 1 {
        return prefix;
    }
    if title.len() == 1 {
        return title;
    }
    Vec::new()
}

fn newest_live_or_last(sessions: &[TerminalSession]) -> Vec<TerminalSession> {
    let mut best_live: Option<usize> = None;
    let mut best_any = 0;
    for (i, s) in sessions.iter().enumerate() {
        if !created_at_before(s, &sessions[best_any]) {
            best_any = i;
        }
        if s.exited {
            continue;
        }
        if best_live.is_none_or(|b| !created_at_before(s, &sessions[b])) {
            best_live = Some(i);
        }
    }
    vec![sessions[best_live.unwrap_or(best_any)].clone()]
}

fn created_at_before(a: &TerminalSession, b: &TerminalSession) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(&a.created_at),
        chrono::DateTime::parse_from_rfc3339(&b.created_at),
    ) {
        (Ok(ta), Ok(tb)) => ta < tb,
        _ => false,
    }
}

/// `, exited (N)` for a dead shell, else "".
pub fn terminal_exited_note(info: &TerminalSession) -> String {
    if !info.exited {
        return String::new();
    }
    format!(", exited ({})", info.exit_code.unwrap_or(0))
}

#[cfg(test)]
#[path = "terminals_tests.rs"]
mod tests;
