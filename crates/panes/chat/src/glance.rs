//! Glanceable live turns (`lib/chat-glance.ts`): the first thinking window stays open, inner
//! narration and subagent notices fold into the step group once more tools follow.

use recur_engine::view::chat::AgentMsg;

/// Seconds the first thinking window stays open before collapsing like a settled turn.
pub const FIRST_CHECKPOINT_SEC: i64 = 20;

/// `isFirstCheckpoint`: sending, under the cap, and every live step is thinking.
pub fn is_first_checkpoint(sending: bool, elapsed_sec: i64, rows: &[&AgentMsg]) -> bool {
    if !sending || elapsed_sec >= FIRST_CHECKPOINT_SEC || rows.is_empty() {
        return false;
    }
    rows.iter().all(|r| r.thinking && r.tool.is_empty())
}

/// `liveGroupOpen`: a missing toggle opens only during the first thinking window.
pub fn live_group_open(explicit: Option<bool>, first_checkpoint: bool) -> bool {
    explicit.unwrap_or(first_checkpoint)
}

/// `hasLaterStep`: a tool, thought or fallback follows `i` in the same turn.
pub fn has_later_step(list: &[AgentMsg], i: usize) -> bool {
    for m in list.iter().skip(i + 1) {
        if m.me || m.turn_end || m.log.is_some() {
            break;
        }
        if !m.tool.is_empty() || m.thinking || m.fallback {
            return true;
        }
    }
    false
}

/// `foldInnerNarration`: settled "I'll look next" prose hides once more tools follow.
pub fn fold_inner_narration(m: &AgentMsg, later_step: bool) -> bool {
    later_step && !m.streaming
}

/// `foldSubagentNotice`: Subagent notes fold into the following steps.
pub fn fold_subagent_notice(m: &AgentMsg, later_step: bool) -> bool {
    m.note && m.note_label == "Subagent" && later_step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(f: impl FnOnce(&mut AgentMsg)) -> AgentMsg {
        let mut m = AgentMsg::default();
        f(&mut m);
        m
    }

    // chat-glance.test.ts
    #[test]
    fn first_checkpoint() {
        let think = m(|m| m.thinking = true);
        let read = m(|m| m.tool = "Read".into());
        assert!(is_first_checkpoint(true, 5, &[&think]));
        assert!(!is_first_checkpoint(true, 5, &[&think, &read]));
        assert!(!is_first_checkpoint(true, FIRST_CHECKPOINT_SEC, &[&think]));
        assert!(!is_first_checkpoint(false, 1, &[&think]));
        assert!(!is_first_checkpoint(true, 1, &[]));
    }

    #[test]
    fn group_open() {
        assert!(live_group_open(None, true));
        assert!(!live_group_open(None, false));
        assert!(!live_group_open(Some(false), true));
        assert!(live_group_open(Some(true), false));
    }

    #[test]
    fn inner_narration_folds() {
        let turn = vec![
            m(|m| m.me = true),
            m(|m| m.body = "plan".into()),
            m(|m| m.tool = "Read".into()),
            m(|m| m.body = "inner".into()),
            m(|m| m.tool = "Edit".into()),
            m(|m| m.body = "final".into()),
            m(|m| m.turn_end = true),
        ];
        assert!(fold_inner_narration(&turn[1], has_later_step(&turn, 1)));
        assert!(!fold_inner_narration(
            &m(|m| m.streaming = true),
            has_later_step(&turn, 3)
        ));
        assert!(fold_inner_narration(&turn[3], has_later_step(&turn, 3)));
        assert!(!fold_inner_narration(&turn[5], has_later_step(&turn, 5)));
        let turn = vec![
            m(|m| m.me = true),
            m(|m| m.tool = "Read".into()),
            m(|m| m.thinking = true),
            m(|m| m.body = "final".into()),
            m(|m| m.compact = true),
            m(|m| m.turn_end = true),
        ];
        assert!(!fold_inner_narration(&turn[3], has_later_step(&turn, 3)));
        assert!(has_later_step(&turn, 1));
    }

    #[test]
    fn subagent_notices_fold() {
        let note = |l: &str| {
            m(|m| {
                m.note = true;
                m.note_label = l.into();
            })
        };
        let turn = vec![
            note("Subagent"),
            m(|m| m.tool = "Read".into()),
            note("Subagent"),
            m(|m| m.thinking = true),
            note("Commit"),
            m(|m| m.tool = "Edit".into()),
            note("Handoff"),
            m(|m| m.tool = "Grep".into()),
            note("Subagent"),
            m(|m| m.turn_end = true),
        ];
        assert!(fold_subagent_notice(&turn[0], has_later_step(&turn, 0)));
        assert!(fold_subagent_notice(&turn[2], has_later_step(&turn, 2)));
        assert!(!fold_subagent_notice(&turn[4], has_later_step(&turn, 4)));
        assert!(!fold_subagent_notice(&turn[6], has_later_step(&turn, 6)));
        assert!(!fold_subagent_notice(&turn[8], has_later_step(&turn, 8)));
        assert!(!fold_subagent_notice(&note("Subagent"), false));
    }
}
