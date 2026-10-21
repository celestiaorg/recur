//! The transcript's structure (`AgentChat.svelte`): rows grouped into blocks (contiguous steps
//! form one collapsible group), their stable keys, group summaries, the retry gate, the
//! queued-follow-up split, and a user bubble's inline parts.

use std::collections::BTreeSet;

use recur_engine::view::chat::{AgentMsg, ChatAttachmentView, ChatLog};
use recur_engine::view::composer::is_dir_chip_path;
use recur_engine::view::mentions::{
    artifact_at_hits, attachment_hits, attachment_marker, collect_loose_at_names, file_at_hits,
    skill_slash_hits, terminal_at_hits, token_span_overlaps,
};

use crate::glance::{fold_inner_narration, fold_subagent_notice, has_later_step};
use crate::tools::{ToolCx, ToolView};
use crate::transcript_ui::{
    transcript_block_key, transcript_message_block_local, transcript_session_block_local,
    transcript_step_block_local, unique_block_keys,
};

/// A transcript block: one row, or a run of step rows (tools, thoughts, compactions,
/// fallbacks, folded narration and subagent notices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Msg(usize),
    Steps { start: usize, rows: Vec<usize> },
}

/// `blocks`: indices are into the transcript (queued rows excluded).
pub fn group_blocks(msgs: &[AgentMsg]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut steps: Option<usize> = None;
    for (i, m) in msgs.iter().enumerate() {
        let push_step = |out: &mut Vec<Block>, steps: &mut Option<usize>| match steps {
            Some(at) => {
                if let Some(Block::Steps { rows, .. }) = out.get_mut(*at) {
                    rows.push(i);
                }
            }
            None => {
                out.push(Block::Steps {
                    start: i,
                    rows: vec![i],
                });
                *steps = Some(out.len() - 1);
            }
        };
        if m.log.is_some() {
            steps = None;
            out.push(Block::Msg(i));
        } else if !m.tool.is_empty()
            && ToolView::new(m, ToolCx::default()).is_subagent_create()
        {
            continue;
        } else if ((!m.tool.is_empty() || m.thinking || m.compact || m.fallback) && !m.turn_end)
            || fold_subagent_notice(m, has_later_step(msgs, i))
        {
            push_step(&mut out, &mut steps);
        } else if m.me || m.turn_end || m.note || m.stopped || m.error {
            steps = None;
            out.push(Block::Msg(i));
        } else if steps.is_some() && fold_inner_narration(m, has_later_step(msgs, i)) {
            push_step(&mut out, &mut steps);
        } else {
            out.push(Block::Msg(i));
        }
    }
    out
}

/// `blockKey` for every block, made unique.
pub fn block_keys(reset_key: &str, blocks: &[Block], msgs: &[AgentMsg]) -> Vec<String> {
    let n = blocks.len();
    let keys = blocks
        .iter()
        .enumerate()
        .map(|(bi, b)| match b {
            Block::Steps { start, rows } => {
                let first = rows.first().map(|&r| &msgs[r]);
                let live = rows
                    .iter()
                    .any(|&r| msgs[r].streaming || msgs[r].result_pending);
                let local = transcript_step_block_local(
                    *start,
                    first.map_or("", |m| m.tool_id.as_str()),
                    first.and_then(|m| m.ts),
                    first.map_or("", |m| m.body.as_str()),
                    live && bi + 1 == n,
                );
                transcript_block_key(reset_key, &local)
            }
            Block::Msg(i) => {
                let m = &msgs[*i];
                let local = match m.log.as_deref() {
                    Some(log @ (ChatLog::Session(_) | ChatLog::Sessions { .. })) => {
                        transcript_session_block_local(log)
                    }
                    Some(ChatLog::Fact { id, text, .. }) => {
                        let tail = if id.is_empty() {
                            format!(
                                "{}:{}",
                                ts_string(m.ts),
                                crate::transcript_ui::js_prefix(text, 48)
                            )
                        } else {
                            id.clone()
                        };
                        format!("fact:{tail}")
                    }
                    None if !m.pending_id.is_empty() => format!("q:{}", m.pending_id),
                    None if m.streaming => {
                        if m.thinking { "m:live-think" } else { "m:live-text" }.to_string()
                    }
                    None => transcript_message_block_local(*i, m.me, m.ts, &m.tool_id, &m.body),
                };
                transcript_block_key(reset_key, &local)
            }
        })
        .collect();
    unique_block_keys(keys)
}

/// `${m.ts}` as JS prints an optional number (`undefined` when unset).
fn ts_string(ts: Option<i64>) -> String {
    ts.map_or_else(|| "undefined".to_string(), |t| t.to_string())
}

/// `groupSummary`: `3 tool calls · 1 thought · 2 files · compaction`.
pub fn group_summary(
    msgs: &[AgentMsg],
    head_rows: &[usize],
    file_rows: &[usize],
    cx: ToolCx<'_>,
) -> String {
    let count = |f: fn(&AgentMsg) -> bool| head_rows.iter().filter(|&&r| f(&msgs[r])).count();
    let tools = count(|m| !m.tool.is_empty());
    let compacts = count(|m| m.compact);
    let fallbacks = count(|m| m.fallback);
    let thoughts = count(|m| m.thinking);
    let mut files = BTreeSet::new();
    for &r in file_rows {
        let f = ToolView::new(&msgs[r], cx).file_of();
        if !f.is_empty() {
            files.insert(f);
        }
    }
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if tools > 0 {
        parts.push(format!("{tools} tool call{}", s(tools)));
    }
    if thoughts > 0 {
        parts.push(format!("{thoughts} thought{}", s(thoughts)));
    }
    if !files.is_empty() {
        parts.push(format!("{} file{}", files.len(), s(files.len())));
    }
    if compacts > 0 {
        parts.push(if compacts == 1 {
            "compaction".into()
        } else {
            format!("{compacts} compactions")
        });
    }
    if fallbacks > 0 {
        parts.push(if fallbacks == 1 {
            "model fallback".into()
        } else {
            format!("{fallbacks} model fallbacks")
        });
    }
    if parts.is_empty() {
        "steps".into()
    } else {
        parts.join(" · ")
    }
}

/// `lastRetryErrorIndex`: Retry only on the trailing error; later text means the turn went on.
pub fn last_retry_error_index(
    msgs: &[AgentMsg],
    readonly: bool,
    sending: bool,
    has_retry: bool,
) -> Option<usize> {
    if readonly || sending || !has_retry {
        return None;
    }
    for (i, m) in msgs.iter().enumerate().rev() {
        if m.me {
            return None;
        }
        if m.error {
            return Some(i);
        }
        if m.log.is_some()
            || m.turn_end
            || !m.tool.is_empty()
            || m.thinking
            || m.compact
            || m.note
            || m.fallback
            || m.stopped
        {
            continue;
        }
        if !m.body.is_empty() {
            return None;
        }
    }
    None
}

/// `cockpitStartTs`: the earliest real stamp (ts 1 is the synthetic opening prompt), 0 if none.
pub fn cockpit_start_ts(msgs: &[AgentMsg]) -> i64 {
    msgs.iter()
        .filter_map(|m| m.ts)
        .filter(|&t| t > 1)
        .min()
        .unwrap_or(0)
}

/// One inline part of a user bubble (`BubblePart`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BubblePart {
    Text(String),
    Image { id: String, src: String, name: String },
    File { path: String, dir: bool },
    Skill(String),
    Terminal(String),
    Artifact(String),
}

/// `splitInlineTokens`: `@artifact`, `@terminal`, `@file`, `/skill`, then loose `@Title`
/// artifacts (last, so overlaps match the composer's hydrate).
pub fn split_inline_tokens(text: &str, artifact_id_for_title: &dyn Fn(&str) -> String) -> Vec<BubblePart> {
    let mut toks: Vec<(usize, usize, BubblePart)> = Vec::new();
    for h in artifact_at_hits(text) {
        toks.push((h.start, h.end - h.start, BubblePart::Artifact(h.body.unwrap_or("").into())));
    }
    for h in terminal_at_hits(text) {
        toks.push((h.start, h.end - h.start, BubblePart::Terminal(h.body.unwrap_or("").into())));
    }
    for h in file_at_hits(text) {
        if let Some(p) = h.body {
            toks.push((
                h.start,
                h.end - h.start,
                BubblePart::File {
                    path: p.into(),
                    dir: is_dir_chip_path(p),
                },
            ));
        }
    }
    for h in skill_slash_hits(text) {
        if let Some(n) = h.body {
            toks.push((h.start, 1 + n.len(), BubblePart::Skill(n.into())));
        }
    }
    for hit in collect_loose_at_names(text) {
        let id = artifact_id_for_title(&hit.token);
        if id.is_empty() {
            continue;
        }
        let spans: Vec<(usize, usize)> = toks.iter().map(|t| (t.0, t.1)).collect();
        if token_span_overlaps(hit.index, hit.len, &spans) {
            continue;
        }
        toks.push((hit.index, hit.len, BubblePart::Artifact(id)));
    }
    toks.sort_by_key(|t| t.0);
    let mut parts = Vec::new();
    let mut last = 0;
    for (index, len, part) in toks {
        if index < last {
            continue;
        }
        if index > last {
            parts.push(BubblePart::Text(text[last..index].to_string()));
        }
        parts.push(part);
        last = index + len;
    }
    if last < text.len() {
        parts.push(BubblePart::Text(text[last..].to_string()));
    }
    if parts.is_empty() && !text.is_empty() {
        parts.push(BubblePart::Text(text.to_string()));
    }
    parts
}

fn att_src(a: &ChatAttachmentView) -> &str {
    if a.data_url.is_empty() { &a.url } else { &a.data_url }
}

/// `bubbleParts`: attachment markers become images where the row carries them; images the
/// text never placed follow at the end.
pub fn bubble_parts(m: &AgentMsg, artifact_id_for_title: &dyn Fn(&str) -> String) -> Vec<BubblePart> {
    let body = m.body.as_str();
    let mut parts = Vec::new();
    let mut last = 0;
    for h in attachment_hits(body) {
        if h.start > last {
            parts.extend(split_inline_tokens(&body[last..h.start], artifact_id_for_title));
        }
        let id = h.body.unwrap_or("");
        let att = m.attachments.iter().find(|a| a.id == id);
        let src = att.map_or("", att_src);
        if src.is_empty() {
            parts.push(BubblePart::Text(body[h.start..h.end].to_string()));
        } else {
            let name = att.map(|a| a.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(id);
            parts.push(BubblePart::Image {
                id: id.into(),
                src: src.into(),
                name: name.into(),
            });
        }
        last = h.end;
    }
    if last < body.len() {
        parts.extend(split_inline_tokens(&body[last..], artifact_id_for_title));
    }
    if parts.is_empty() && !body.is_empty() {
        parts.extend(split_inline_tokens(body, artifact_id_for_title));
    }
    for a in &m.attachments {
        if body.contains(&attachment_marker(&a.id)) {
            continue;
        }
        let src = att_src(a);
        if !src.is_empty() {
            parts.push(BubblePart::Image {
                id: a.id.clone(),
                src: src.into(),
                name: if a.name.is_empty() { a.id.clone() } else { a.name.clone() },
            });
        }
    }
    parts
}

/// `lastPresentedInTurn`: the last successful `present_workspace` card in the turn that ends
/// at `turn_end_index`.
pub fn last_presented_in_turn(
    msgs: &[AgentMsg],
    turn_end_index: usize,
    cx: ToolCx<'_>,
) -> Option<recur_agent_tools::display::briefing_tools::PresentWorkspaceView> {
    if turn_end_index == 0 {
        return None;
    }
    for m in msgs[..turn_end_index.min(msgs.len())].iter().rev() {
        if m.me || m.turn_end {
            break;
        }
        let v = ToolView::new(m, cx);
        if !v.is_present_workspace() || m.result_error || m.result_pending {
            continue;
        }
        if let Some(card) = v.present_workspace_result()
            && card.ok
            && (card.brief.is_some() || !card.target.is_empty())
        {
            return Some(card);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(f: impl FnOnce(&mut AgentMsg)) -> AgentMsg {
        let mut x = AgentMsg::default();
        f(&mut x);
        x
    }

    #[test]
    fn steps_group_and_fold() {
        // AgentChat.test.ts: Subagent notices fold into the following steps group.
        let msgs = vec![
            m(|x| {
                x.note = true;
                x.note_label = "Subagent".into();
                x.body = "Goal finished (child-1)".into();
                x.ts = Some(1);
            }),
            m(|x| {
                x.tool = "Read".into();
                x.result = "ok".into();
            }),
            m(|x| {
                x.tool = "Grep".into();
                x.result = "none".into();
            }),
        ];
        let b = group_blocks(&msgs);
        assert_eq!(b, vec![Block::Steps { start: 0, rows: vec![0, 1, 2] }]);
        assert_eq!(group_summary(&msgs, &[0, 1, 2], &[0, 1, 2], ToolCx::default()), "2 tool calls");

        // A final reply stays visible when compaction follows the turn.
        let msgs = vec![
            m(|x| {
                x.me = true;
                x.body = "where is check-in?".into();
            }),
            m(|x| x.tool = "Read".into()),
            m(|x| {
                x.thinking = true;
                x.body = "looking".into();
            }),
            m(|x| x.body = "Check-in only renders when Apollo Sync returns one.".into()),
            m(|x| {
                x.compact = true;
                x.body = "folded".into();
            }),
            m(|x| {
                x.turn_end = true;
                x.duration_ms = 42000;
            }),
        ];
        // The reply is a block of its own; the compaction after it joins the open group.
        let b = group_blocks(&msgs);
        assert_eq!(
            b,
            vec![
                Block::Msg(0),
                Block::Steps { start: 1, rows: vec![1, 2, 4] },
                Block::Msg(3),
                Block::Msg(5),
            ]
        );
        assert!(group_summary(&msgs, &[1, 2, 4], &[1, 2, 4], ToolCx::default()).contains("compaction"));
    }

    #[test]
    fn retry_only_on_the_trailing_error() {
        let err = m(|x| {
            x.error = true;
            x.body = "Agent error: boom".into();
        });
        let me = m(|x| {
            x.me = true;
            x.body = "go".into();
        });
        let foot = m(|x| x.turn_end = true);
        let reply = m(|x| x.body = "later".into());
        assert_eq!(last_retry_error_index(&[me.clone(), err.clone(), foot.clone()], false, false, true), Some(1));
        assert_eq!(last_retry_error_index(&[err.clone(), reply], false, false, true), None);
        assert_eq!(last_retry_error_index(&[err.clone(), me], false, false, true), None);
        assert_eq!(last_retry_error_index(&[err.clone()], false, true, true), None);
        assert_eq!(last_retry_error_index(&[err], true, false, true), None);
    }

    #[test]
    fn bubble_parts_split_chips_and_images() {
        let none = |_: &str| String::new();
        let msg = m(|x| {
            x.me = true;
            x.body = "how does @PROGRESS.md compare to @src/ and /land-it ⟦attachment:i1⟧ @terminal:t1".into();
            x.attachments = vec![
                ChatAttachmentView {
                    id: "i1".into(),
                    data_url: "data:image/png;base64,AA".into(),
                    name: "shot.png".into(),
                    ..Default::default()
                },
                ChatAttachmentView {
                    id: "i2".into(),
                    url: "/chat-image?x".into(),
                    ..Default::default()
                },
            ];
        });
        let parts = bubble_parts(&msg, &none);
        assert_eq!(parts[0], BubblePart::Text("how does ".into()));
        assert_eq!(parts[1], BubblePart::File { path: "PROGRESS.md".into(), dir: false });
        assert_eq!(parts[3], BubblePart::File { path: "src/".into(), dir: true });
        assert_eq!(parts[5], BubblePart::Skill("land-it".into()));
        assert!(matches!(&parts[7], BubblePart::Image { name, .. } if name == "shot.png"));
        assert_eq!(parts[9], BubblePart::Terminal("t1".into()));
        assert!(matches!(parts.last(), Some(BubblePart::Image { id, .. }) if id == "i2"));
        let titled = |t: &str| if t == "Notes" { "art-1".to_string() } else { String::new() };
        let parts = split_inline_tokens("review @Notes now", &titled);
        assert_eq!(parts[1], BubblePart::Artifact("art-1".into()));
    }

    #[test]
    fn keys_are_stable_and_unique() {
        let msgs = vec![
            m(|x| {
                x.me = true;
                x.body = "hi".into();
                x.ts = Some(100);
            }),
            m(|x| {
                x.tool = "Bash".into();
                x.tool_id = "t1".into();
            }),
            m(|x| {
                x.body = "reply".into();
                x.streaming = true;
            }),
        ];
        let b = group_blocks(&msgs);
        let k = block_keys("ws\tPA", &b, &msgs);
        assert_eq!(k, ["ws\tPA:me:100:hi", "ws\tPA:s:t1", "ws\tPA:m:live-text"]);
        assert_eq!(cockpit_start_ts(&msgs), 100);
    }
}
