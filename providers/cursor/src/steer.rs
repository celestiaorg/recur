//! Steers waiting for delivery.
//!
//! Cursor joins every waiting steer into one message: one SteerRun, or one
//! CancelRun plus one follow-up Send. `seq`/`acked` let the main loop fold a
//! batch into a follow-up prompt while the listener is mid-wait; without the
//! ack the listener's CancelRun would hit the new run. `reserved` is a batch
//! owned by an in-flight SteerRun, so a take cannot fold or ack it twice.
//! Each steer carries its own delivery ack.

use parking_lot::Mutex;
use recur_agent::turn::follow_up_text;
use recur_agent::{AgentEvent, Steer};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    msgs: Vec<Steer>,
    seq: u64,
    acked: u64,
    reserved: Vec<Steer>,
    res_seq: u64,
}

#[derive(Default)]
pub(crate) struct SteerBuf {
    state: Mutex<State>,
    /// Woken when `reserved` clears.
    idle: Notify,
}

/// A batch handed to SteerRun: the joined text and the user rows it shows
/// once delivered live.
pub(crate) struct Reserved {
    pub joined: String,
    pub users: Vec<AgentEvent>,
}

impl SteerBuf {
    pub(crate) fn add(&self, s: Steer) {
        let mut g = self.state.lock();
        g.seq += 1;
        g.msgs.push(s);
    }

    /// Every waiting steer, acking the batch unless one is reserved.
    pub(crate) fn take(&self) -> Vec<Steer> {
        let mut g = self.state.lock();
        if g.reserved.is_empty() {
            g.acked = g.seq;
        }
        std::mem::take(&mut g.msgs)
    }

    pub(crate) fn batch_seq(&self) -> u64 {
        self.state.lock().seq
    }

    /// Whether a take or commit already covered the batch.
    pub(crate) fn consumed(&self, batch: u64) -> bool {
        batch != 0 && self.state.lock().acked >= batch
    }

    /// Takes exclusive ownership of the waiting steers for SteerRun.
    pub(crate) fn reserve(&self) -> Option<Reserved> {
        let mut g = self.state.lock();
        if !g.reserved.is_empty() || g.msgs.is_empty() {
            return None;
        }
        g.res_seq = g.seq;
        g.reserved = std::mem::take(&mut g.msgs);
        let joined = follow_up_text(&g.reserved);
        let users = follow_up_users(&g.reserved, &joined, true);
        Some(Reserved { joined, users })
    }

    /// The reserved batch was delivered: ack it.
    pub(crate) fn commit(&self) {
        let reserved = {
            let mut g = self.state.lock();
            g.acked = g.res_seq;
            std::mem::take(&mut g.reserved)
        };
        for s in reserved {
            s.ack(true);
        }
        self.idle.notify_waiters();
    }

    /// The reserved batch was not delivered: back in front of later steers.
    pub(crate) fn release(&self) {
        {
            let mut g = self.state.lock();
            let mut back = std::mem::take(&mut g.reserved);
            back.append(&mut g.msgs);
            g.msgs = back;
        }
        self.idle.notify_waiters();
    }

    /// Waits until no batch is reserved, or `cancel` fires.
    pub(crate) async fn wait_reserved(&self, cancel: &CancellationToken) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.lock().reserved.is_empty() {
                return;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    /// Drops every waiting and reserved steer as not delivered; returns how
    /// many.
    pub(crate) fn abandon(&self) -> usize {
        let dropped = {
            let mut g = self.state.lock();
            g.acked = g.seq;
            let mut all = std::mem::take(&mut g.msgs);
            all.append(&mut g.reserved);
            all
        };
        let n = dropped.len();
        ack_all(dropped, false);
        self.idle.notify_waiters();
        n
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<String> {
        self.state
            .lock()
            .msgs
            .iter()
            .map(|s| s.prompt.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn take_prefix(&self, n: usize) -> Vec<Steer> {
        let mut g = self.state.lock();
        let n = n.min(g.msgs.len());
        let out: Vec<Steer> = g.msgs.drain(..n).collect();
        if g.msgs.is_empty() {
            g.acked = g.seq;
        }
        out
    }
}

/// Answers each steer's delivery.
pub(crate) fn ack_all(steers: Vec<Steer>, delivered: bool) {
    for s in steers {
        s.ack(delivered);
    }
}

/// The user rows for a follow-up Send or a live delivered steer: one per
/// plain (workspace chat) steer, or one Maestro-prefixed row matching the
/// joined message.
pub(crate) fn follow_up_users(steers: &[Steer], joined: &str, live: bool) -> Vec<AgentEvent> {
    if joined.is_empty() {
        return Vec::new();
    }
    if !steers.is_empty() && steers.iter().all(Steer::is_plain) {
        return steers.iter().filter_map(|s| s.user_event(live)).collect();
    }
    vec![AgentEvent::User {
        text: joined.to_owned(),
        display: String::new(),
        live_steer: live,
    }]
}

/// A steer with nothing to say.
pub(crate) fn blank(s: &Steer) -> bool {
    s.prompt.trim().is_empty() && s.display.as_deref().is_none_or(|d| d.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steer(p: &str) -> Steer {
        Steer::new(p, None).0
    }

    #[test]
    fn take_acks_batch() {
        let s = SteerBuf::default();
        s.add(steer("first"));
        let batch = s.batch_seq();
        assert!(!s.consumed(batch));
        let got = s.take();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].prompt, "first");
        assert!(
            s.consumed(batch),
            "take must ack the folded batch so CancelRun is skipped"
        );
        s.add(steer("second"));
        assert!(
            !s.consumed(s.batch_seq()),
            "a new add must not inherit the previous ack"
        );
    }

    #[test]
    fn take_prefix_leaves_remainder() {
        let s = SteerBuf::default();
        s.add(steer("first"));
        s.add(steer("second"));
        let got = s.take_prefix(1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].prompt, "first");
        assert!(!s.consumed(s.batch_seq()));
        assert_eq!(s.snapshot(), ["second"]);
        let _ = s.take_prefix(1);
        assert!(s.consumed(s.batch_seq()));
    }

    #[test]
    fn reserve_excludes_take() {
        let s = SteerBuf::default();
        let (first, first_ack) = Steer::new("first", None);
        s.add(first);
        let r = s.reserve().unwrap();
        assert_eq!(
            r.joined,
            recur_agent::turn::format_steer_user_message("first")
        );
        assert!(s.take().is_empty());
        assert!(
            !s.consumed(s.batch_seq()),
            "take must not ack a reserved batch"
        );
        s.commit();
        assert!(
            s.consumed(s.batch_seq()),
            "commit must ack the reserved batch"
        );
        assert_eq!(first_ack.blocking_recv(), Ok(true));

        s.add(steer("second"));
        let _ = s.reserve();
        s.add(steer("third"));
        s.release();
        let rest: Vec<String> = s.take().into_iter().map(|x| x.prompt).collect();
        assert_eq!(rest, ["second", "third"]);
    }

    #[test]
    fn abandon_nacks_waiting_and_reserved() {
        let s = SteerBuf::default();
        let (a, a_ack) = Steer::new("a", None);
        let (b, b_ack) = Steer::new("b", None);
        s.add(a);
        let _ = s.reserve();
        s.add(b);
        assert_eq!(s.abandon(), 2);
        assert_eq!(a_ack.blocking_recv(), Ok(false));
        assert_eq!(b_ack.blocking_recv(), Ok(false));
        assert!(s.consumed(s.batch_seq()));
    }

    // The joined follow-up text and its user rows.
    #[test]
    fn joined_text_and_users() {
        let mixed = [steer("  first  "), steer(""), steer("second")];
        let joined = follow_up_text(&mixed);
        assert_eq!(
            joined,
            recur_agent::turn::format_steer_user_message("first\n\nsecond")
        );
        assert_eq!(follow_up_text(&[]), "");

        let plain = [
            Steer::new("  first  ", Some(String::new())).0,
            Steer::new("second", Some(String::new())).0,
        ];
        assert_eq!(follow_up_text(&plain), "first\n\nsecond");

        let wire = Steer::new("WIRE", Some("Fix findings".into())).0;
        let users = follow_up_users(std::slice::from_ref(&wire), "WIRE", true);
        assert_eq!(
            users,
            [AgentEvent::User {
                text: "WIRE".into(),
                display: "Fix findings".into(),
                live_steer: true
            }]
        );
        let maestro = follow_up_users(&[steer("steer now")], "joined", false);
        assert_eq!(
            maestro,
            [AgentEvent::User {
                text: "joined".into(),
                display: String::new(),
                live_steer: false
            }]
        );
    }
}
