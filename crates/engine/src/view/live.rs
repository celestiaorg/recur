//! Live vs settled specialist cards in the Loop cockpit.
//!
//! Stream events (delta/step/turn_end) can still land after `agent.updated`
//! idle. Settled latches only on idle: `turn_end` is per provider run and a
//! coder can emit several inside one working window, so clearing live without
//! latching lets the next nudge's deltas re-light. After idle,
//! [`SessionLiveFlags::touch_from_stream`] stays dark.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct SessionLiveFlags {
    live: BTreeSet<String>,
    settled: BTreeSet<String>,
}

impl SessionLiveFlags {
    pub fn snapshot(&self) -> BTreeSet<String> {
        self.live.clone()
    }

    pub fn reset(&mut self) {
        self.live.clear();
        self.settled.clear();
    }

    /// The engine says this specialist is mid-turn (`agent.updated` /
    /// agentsRunning).
    pub fn mark_running(&mut self, code: &str) -> bool {
        if code.is_empty() {
            return false;
        }
        self.settled.remove(code);
        self.live.insert(code.to_owned())
    }

    /// `agent.updated` idle, the one terminal signal. Late stream events stay
    /// dark.
    pub fn mark_settled(&mut self, code: &str) -> bool {
        if code.is_empty() {
            return false;
        }
        self.settled.insert(code.to_owned());
        self.live.remove(code)
    }

    /// `turn_end` of one provider run. Does not latch: the next run may still
    /// be in the same working window.
    pub fn clear_live(&mut self, code: &str) -> bool {
        !code.is_empty() && self.live.remove(code)
    }

    /// Optimistic live from a stream event before `agent.updated` working.
    pub fn touch_from_stream(&mut self, code: &str) -> bool {
        if code.is_empty() || self.settled.contains(code) {
            return false;
        }
        self.live.insert(code.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(f: &SessionLiveFlags) -> Vec<String> {
        f.snapshot().into_iter().collect()
    }

    #[test]
    fn stream_events_light_a_card_before_working() {
        let mut f = SessionLiveFlags::default();
        assert!(f.touch_from_stream("CO"));
        assert_eq!(codes(&f), ["CO"]);
        assert!(!f.touch_from_stream("CO"));
    }

    #[test]
    fn idle_then_late_stream_or_turn_end_must_not_relight() {
        let mut f = SessionLiveFlags::default();
        f.mark_running("CO");
        f.mark_settled("CO");
        assert!(f.snapshot().is_empty());
        assert!(!f.touch_from_stream("CO"));
        assert!(!f.clear_live("CO"));
        assert!(!f.mark_settled("CO"));
        assert!(f.snapshot().is_empty());
    }

    #[test]
    fn turn_end_without_idle_clears_live_and_the_next_stream_relights() {
        let mut f = SessionLiveFlags::default();
        f.touch_from_stream("CO");
        assert!(f.clear_live("CO"));
        assert!(f.snapshot().is_empty());
        assert!(f.touch_from_stream("CO"));
        assert_eq!(codes(&f), ["CO"]);
    }

    #[test]
    fn a_later_running_status_starts_a_new_live_turn() {
        let mut f = SessionLiveFlags::default();
        f.mark_running("CO");
        f.mark_settled("CO");
        assert!(f.mark_running("CO"));
        assert_eq!(codes(&f), ["CO"]);
        assert!(!f.touch_from_stream("CO"));
    }

    #[test]
    fn reset_drops_live_and_settled() {
        let mut f = SessionLiveFlags::default();
        f.mark_running("CO");
        f.mark_settled("CO");
        f.reset();
        assert!(f.touch_from_stream("CO"));
        assert_eq!(codes(&f), ["CO"]);
    }

    #[test]
    fn empty_codes_are_ignored() {
        let mut f = SessionLiveFlags::default();
        assert!(!f.mark_running(""));
        assert!(!f.mark_settled(""));
        assert!(!f.clear_live(""));
        assert!(!f.touch_from_stream(""));
    }
}
