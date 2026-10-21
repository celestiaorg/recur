//! Chat key remaps when a PR chat host is hoisted onto a live worktree
//! (`lib/hoist-chat-keys.ts`): host-prefixed drafts, queues and in-flight sends move onto the
//! destination workspace's tab codes, and late events addressed to the host follow the alias.
//! The module state is a GPUI global ([`HoistState`]).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui_kit::{App, Global};
use recur_engine::view::chat::ChatAttachmentDraft;

use crate::heap::{ChatHeapCache, PendingChatSend};

pub type CodeRemap = BTreeMap<String, String>;

/// `destChatCode`.
pub fn dest_chat_code<'a>(code: &'a str, remap: Option<&'a CodeRemap>) -> &'a str {
    remap
        .and_then(|r| r.get(code))
        .filter(|c| !c.is_empty())
        .map_or(code, String::as_str)
}

/// `hoistCacheBustCodes`: the destination codes whose parked caches belong to another session
/// after a hoist. `remap == None` is not a hoist; an identity remap names nobody, so every tab
/// busts.
pub fn hoist_cache_bust_codes(
    tab_codes: &[String],
    remap: Option<&CodeRemap>,
    moved_codes: &[String],
) -> Vec<String> {
    let Some(remap) = remap else {
        return Vec::new();
    };
    let mut named: Vec<String> = Vec::new();
    for c in remap.values().chain(moved_codes) {
        if !c.is_empty() && !named.contains(c) {
            named.push(c.clone());
        }
    }
    if named.is_empty() {
        tab_codes.to_vec()
    } else {
        named
    }
}

/// `chatTabKey`: `workspaceId\tcode`.
pub fn chat_tab_key(workspace_id: &str, code: &str) -> String {
    format!("{workspace_id}\t{code}")
}

/// `destChatKey`.
pub fn dest_chat_key(code: &str, to_id: &str, remap: Option<&CodeRemap>) -> String {
    chat_tab_key(to_id, dest_chat_code(code, remap))
}

/// `concatHoistText`.
pub fn concat_hoist_text(dest: &str, src: &str) -> String {
    let (a, b) = (dest.trim(), src.trim());
    if b.is_empty() {
        return dest.to_string();
    }
    if a.is_empty() {
        return src.to_string();
    }
    format!("{dest}\n\n{src}")
}

/// `concatHoistList`.
pub fn concat_hoist_list<T: Clone>(dest: &[T], src: &[T]) -> Vec<T> {
    let mut out = dest.to_vec();
    out.extend_from_slice(src);
    out
}

/// A draft stashed for a destination tab that is not mounted yet (`HoistDraft`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoistDraft {
    pub input: String,
    pub attachments: Vec<ChatAttachmentDraft>,
}

/// Queued sends stashed for the destination (`HoistPending`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoistPending {
    pub items: Vec<PendingChatSend>,
    pub hold: bool,
}

type SendComplete = Rc<dyn Fn(&str, u64)>;

/// The hoist module state: aliases, stashes, send reservations and their listeners.
#[derive(Default)]
pub struct HoistState {
    aliases: BTreeMap<String, String>,
    drafts: BTreeMap<String, HoistDraft>,
    queues: BTreeMap<String, HoistPending>,
    sending: BTreeMap<String, u64>,
    send_complete: Vec<(u64, SendComplete)>,
    next_listener: u64,
    live_sources: BTreeSet<String>,
    pending_dest_bust: BTreeMap<String, BTreeSet<String>>,
}

impl Global for HoistState {}

fn take_prefixed<T>(map: &mut BTreeMap<String, T>, workspace_id: &str) -> BTreeMap<String, T> {
    if workspace_id.is_empty() {
        return BTreeMap::new();
    }
    let p = format!("{workspace_id}\t");
    let keys: Vec<String> = map.keys().filter(|k| k.starts_with(&p)).cloned().collect();
    keys.into_iter()
        .filter_map(|k| map.remove(&k).map(|v| (k, v)))
        .collect()
}

impl HoistState {
    pub fn global(cx: &mut App) -> &mut HoistState {
        cx.default_global::<HoistState>()
    }

    /// `noteHoistChatAlias`.
    pub fn note_alias(&mut self, from_key: &str, to_key: &str) {
        if from_key.is_empty() || to_key.is_empty() || from_key == to_key {
            return;
        }
        self.aliases.insert(from_key.to_string(), to_key.to_string());
    }

    /// `liveChatKey`.
    pub fn live_chat_key(&self, key: &str) -> String {
        self.aliases
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// `liveChatTarget`: where events addressed to `(id, code)` now land.
    pub fn live_chat_target(&self, id: &str, code: &str) -> (String, String, String) {
        let key = self.live_chat_key(&chat_tab_key(id, code));
        match key.find('\t') {
            Some(i) if i > 0 => (key[..i].to_string(), key[i + 1..].to_string(), key),
            _ => (id.to_string(), code.to_string(), chat_tab_key(id, code)),
        }
    }

    /// `remapPrefixedRecord`: move `from_id`-prefixed keys onto `to_id` with the code remap,
    /// merging into existing destination values, and note the aliases.
    pub fn remap_prefixed<T: Clone>(
        &mut self,
        rec: &BTreeMap<String, T>,
        from_id: &str,
        to_id: &str,
        remap: Option<&CodeRemap>,
        merge: impl Fn(Option<&T>, &T) -> T,
    ) -> BTreeMap<String, T> {
        let p = format!("{from_id}\t");
        let mut out = rec.clone();
        for (k, v) in rec {
            let Some(code) = k.strip_prefix(&p) else {
                continue;
            };
            let dest = dest_chat_key(code, to_id, remap);
            self.note_alias(k, &dest);
            out.remove(k);
            let merged = merge(out.get(&dest), v);
            out.insert(dest, merged);
        }
        out
    }

    /// `stashHoistDraft`.
    pub fn stash_draft(&mut self, to_key: &str, input: &str, attachments: &[ChatAttachmentDraft]) {
        if to_key.is_empty() || (input.trim().is_empty() && attachments.is_empty()) {
            return;
        }
        let prev = self.drafts.remove(to_key).unwrap_or_default();
        self.drafts.insert(
            to_key.to_string(),
            HoistDraft {
                input: concat_hoist_text(&prev.input, input),
                attachments: concat_hoist_list(&prev.attachments, attachments),
            },
        );
    }

    /// `takeHoistDrafts`.
    pub fn take_drafts(&mut self, workspace_id: &str) -> BTreeMap<String, HoistDraft> {
        take_prefixed(&mut self.drafts, workspace_id)
    }

    /// `noteHoistDestBust`: destination heap keys to drop on remount.
    pub fn note_dest_bust(&mut self, to_id: &str, codes: &[String]) {
        if to_id.is_empty() {
            return;
        }
        let set = self.pending_dest_bust.entry(to_id.to_string()).or_default();
        set.extend(codes.iter().filter(|c| !c.is_empty()).cloned());
    }

    /// `takeHoistDestBust`.
    pub fn take_dest_bust(&mut self, to_id: &str) -> Vec<String> {
        self.pending_dest_bust
            .remove(to_id)
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }

    /// `bustHoistedDestHeap`: drop the destination heap entries for `codes`, keeping their
    /// drafts (then the incoming ones) as stashes.
    pub fn bust_dest_heap(&mut self, heap: &mut ChatHeapCache, to_id: &str, codes: &[String]) {
        for code in codes.iter().filter(|c| !c.is_empty()) {
            let key = chat_tab_key(to_id, code);
            let snap = heap.get(to_id);
            let dest_input = snap
                .as_ref()
                .and_then(|h| h.input.get(&key).cloned())
                .unwrap_or_default();
            let dest_att = snap
                .as_ref()
                .and_then(|h| h.attachments.get(&key).cloned())
                .unwrap_or_default();
            heap.drop_key(to_id, &key);
            let incoming = self.drafts.remove(&key);
            self.stash_draft(&key, &dest_input, &dest_att);
            if let Some(d) = incoming {
                self.stash_draft(&key, &d.input, &d.attachments);
            }
        }
    }

    /// `noteHoistLiveSource`: a mounted workspace view owns its live drafts. Pair with
    /// [`HoistState::drop_live_source`].
    pub fn note_live_source(&mut self, workspace_id: &str) {
        if !workspace_id.is_empty() {
            self.live_sources.insert(workspace_id.to_string());
        }
    }

    pub fn drop_live_source(&mut self, workspace_id: &str) {
        self.live_sources.remove(workspace_id);
    }

    /// `migrateEvictedHoistDrafts`: an evicted host's heap drafts onto destination stashes (a
    /// mounted source already did this itself).
    pub fn migrate_evicted_drafts(
        &mut self,
        heap: &mut ChatHeapCache,
        from_id: &str,
        to_id: &str,
        remap: Option<&CodeRemap>,
    ) {
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return;
        }
        if self.live_sources.contains(from_id) {
            return;
        }
        let (input, attachments) = heap.take_drafts(from_id);
        let p = format!("{from_id}\t");
        let mut seen = BTreeSet::new();
        for (k, v) in &input {
            let Some(code) = k.strip_prefix(&p) else {
                continue;
            };
            seen.insert(k.clone());
            let dest = dest_chat_key(code, to_id, remap);
            self.note_alias(k, &dest);
            let atts = attachments.get(k).cloned().unwrap_or_default();
            self.stash_draft(&dest, v, &atts);
        }
        for (k, v) in &attachments {
            let Some(code) = k.strip_prefix(&p) else {
                continue;
            };
            if seen.contains(k) {
                continue;
            }
            let dest = dest_chat_key(code, to_id, remap);
            self.note_alias(k, &dest);
            self.stash_draft(&dest, "", v);
        }
    }

    /// `stashHoistPending`.
    pub fn stash_pending(&mut self, to_key: &str, items: &[PendingChatSend], hold: bool) {
        if to_key.is_empty() || (items.is_empty() && !hold) {
            return;
        }
        let prev = self.queues.remove(to_key).unwrap_or_default();
        self.queues.insert(
            to_key.to_string(),
            HoistPending {
                items: concat_hoist_list(&prev.items, items),
                hold: prev.hold || hold,
            },
        );
    }

    /// `takeHoistPending`.
    pub fn take_pending(&mut self, workspace_id: &str) -> BTreeMap<String, HoistPending> {
        take_prefixed(&mut self.queues, workspace_id)
    }

    /// `stashHoistSending`: the in-flight send reservation, newest generation wins.
    pub fn stash_sending(&mut self, to_key: &str, gen_: u64) {
        if to_key.is_empty() || gen_ < 1 {
            return;
        }
        let cur = self.sending.get(to_key).copied().unwrap_or(0);
        self.sending.insert(to_key.to_string(), cur.max(gen_));
    }

    /// `takeHoistSending`.
    pub fn take_sending(&mut self, workspace_id: &str) -> BTreeMap<String, u64> {
        take_prefixed(&mut self.sending, workspace_id)
    }

    /// `onHoistSendComplete`: returns a handle for [`HoistState::off_send_complete`].
    pub fn on_send_complete(&mut self, f: impl Fn(&str, u64) + 'static) -> u64 {
        self.next_listener += 1;
        self.send_complete.push((self.next_listener, Rc::new(f)));
        self.next_listener
    }

    pub fn off_send_complete(&mut self, id: u64) {
        self.send_complete.retain(|(i, _)| *i != id);
    }

    /// `clearHoistSending`: a send finished; a stale generation leaves a newer reservation.
    pub fn clear_sending(&mut self, key: &str, gen_: u64) {
        if key.is_empty() {
            return;
        }
        if gen_ != 0
            && let Some(cur) = self.sending.get(key)
            && *cur != gen_
        {
            return;
        }
        self.sending.remove(key);
        if gen_ != 0 {
            let listeners: Vec<SendComplete> =
                self.send_complete.iter().map(|(_, f)| f.clone()).collect();
            for f in listeners {
                f(key, gen_);
            }
        }
    }
}

/// The destination's send bookkeeping after absorbing transferred sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendBooks {
    pub sending: BTreeMap<String, bool>,
    pub send_gen: BTreeMap<String, u64>,
    pub hoist_send_gen: BTreeMap<String, u64>,
}

/// `absorbHoistSending`: merge transferred in-flight sends; the source generation is the
/// reservation unless the destination is already sending.
pub fn absorb_hoist_sending(mut books: SendBooks, inflight: &BTreeMap<String, u64>) -> SendBooks {
    for (k, gen_) in inflight {
        let dest_busy = books.sending.get(k).copied().unwrap_or(false);
        books.sending.insert(k.clone(), true);
        let cur = books.send_gen.get(k).copied().unwrap_or(0);
        books.send_gen.insert(k.clone(), cur.max(*gen_));
        if !dest_busy {
            books.hoist_send_gen.insert(k.clone(), *gen_);
        }
    }
    books
}

/// `hoistSendCompleteApplies`: a source completion matches the transferred reservation.
pub fn hoist_send_complete_applies(complete_gen: u64, _dest_gen: u64, reserved: Option<u64>) -> bool {
    if complete_gen == 0 {
        return true;
    }
    reserved == Some(complete_gen)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::heap::{ChatHeapSnapshot, ChatPickerSnap};
    use recur_engine::view::chat::AgentMsg;
    use recur_view_core::modes::ChatMode;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn remap(pairs: &[(&str, &str)]) -> CodeRemap {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn d(id: &str) -> ChatAttachmentDraft {
        ChatAttachmentDraft {
            id: id.into(),
            ..Default::default()
        }
    }

    fn p(id: &str) -> PendingChatSend {
        PendingChatSend {
            id: id.into(),
            ..Default::default()
        }
    }

    fn body(b: &str) -> AgentMsg {
        AgentMsg {
            body: b.into(),
            ..Default::default()
        }
    }

    // hoist-chat-keys.test.ts
    #[test]
    fn cache_bust_codes() {
        assert!(hoist_cache_bust_codes(&v(&["PA", "PA2"]), None, &[]).is_empty());
        assert_eq!(
            hoist_cache_bust_codes(&v(&["PA", "PA2", "PA3"]), Some(&remap(&[("PA", "PA2")])), &[]),
            v(&["PA2"])
        );
        assert_eq!(
            hoist_cache_bust_codes(
                &v(&["PA", "PA2", "PA3"]),
                Some(&remap(&[("PA", "PA3")])),
                &v(&["PA3", "PA2"])
            ),
            v(&["PA3", "PA2"])
        );
        assert_eq!(
            hoist_cache_bust_codes(&v(&["PA", "PA2"]), Some(&remap(&[])), &[]),
            v(&["PA", "PA2"])
        );
    }

    #[test]
    fn evicted_dest_remount_drops_identity_moved_heap() {
        let mut h = HoistState::default();
        let mut heap = ChatHeapCache::default();
        let picker = ChatPickerSnap {
            model: "claude-code/sonnet".into(),
            flags: Default::default(),
            mode: ChatMode::Agent,
        };
        heap.put(
            "dest",
            &ChatHeapSnapshot {
                chat: [
                    ("dest\tPA".to_string(), vec![body("keep")]),
                    ("dest\tPA2".to_string(), vec![body("placeholder")]),
                ]
                .into_iter()
                .collect(),
                pickers: [
                    ("dest\tPA".to_string(), picker.clone()),
                    ("dest\tPA2".to_string(), picker),
                ]
                .into_iter()
                .collect(),
                input: [
                    ("dest\tPA".to_string(), "leave me".to_string()),
                    ("dest\tPA2".to_string(), "typed on dest".to_string()),
                ]
                .into_iter()
                .collect(),
                attachments: [("dest\tPA2".to_string(), vec![d("d1")])].into_iter().collect(),
                ..Default::default()
            },
        );
        h.stash_draft("dest\tPA2", "from host", &[d("h1")]);
        let codes = hoist_cache_bust_codes(&[], Some(&remap(&[("PA", "PA3")])), &v(&["PA3", "PA2"]));
        h.note_dest_bust("dest", &codes);
        let bust = h.take_dest_bust("dest");
        h.bust_dest_heap(&mut heap, "dest", &bust);
        let hit = heap.get("dest").expect("hit");
        assert!(!hit.pickers.contains_key("dest\tPA2") && !hit.chat.contains_key("dest\tPA2"));
        assert_eq!(hit.pickers["dest\tPA"].mode, ChatMode::Agent);
        assert_eq!(hit.chat["dest\tPA"], vec![body("keep")]);
        assert_eq!(hit.input["dest\tPA"], "leave me");
        let drafts = h.take_drafts("dest");
        assert_eq!(
            drafts,
            [(
                "dest\tPA2".to_string(),
                HoistDraft {
                    input: "typed on dest\n\nfrom host".into(),
                    attachments: vec![d("d1"), d("h1")],
                }
            )]
            .into_iter()
            .collect()
        );
        assert!(h.take_dest_bust("dest").is_empty());
    }

    #[test]
    fn remaps_host_keys_onto_dest() {
        let mut h = HoistState::default();
        let rec: BTreeMap<String, String> = [
            ("host\tPA", "from-host"),
            ("dest\tPA", "on-dest"),
            ("other\tPA", "keep"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let got = h.remap_prefixed(&rec, "host", "dest", Some(&remap(&[("PA", "PA2")])), |a, b| {
            concat_hoist_text(a.map_or("", String::as_str), b)
        });
        let want: BTreeMap<String, String> = [
            ("dest\tPA", "on-dest"),
            ("dest\tPA2", "from-host"),
            ("other\tPA", "keep"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(got, want);
        assert_eq!(h.live_chat_key("host\tPA"), "dest\tPA2");
        assert_eq!(
            h.live_chat_target("host", "PA"),
            ("dest".to_string(), "PA2".to_string(), "dest\tPA2".to_string())
        );
        let pending: BTreeMap<String, Vec<PendingChatSend>> =
            [("host\tPA".to_string(), vec![p("q")])].into_iter().collect();
        let got = h.remap_prefixed(&pending, "host", "dest", Some(&remap(&[])), |a, b| {
            concat_hoist_list(a.map_or(&[][..], Vec::as_slice), b)
        });
        assert_eq!(got, [("dest\tPA".to_string(), vec![p("q")])].into_iter().collect());
        assert_eq!(dest_chat_key("PA", "dest", Some(&remap(&[]))), "dest\tPA");
    }

    #[test]
    fn drafts_for_unmounted_dest() {
        let mut h = HoistState::default();
        h.stash_draft("dest\tPA2", "follow up", &[d("a1")]);
        assert!(h.take_drafts("host").is_empty());
        assert_eq!(
            h.take_drafts("dest")["dest\tPA2"],
            HoistDraft {
                input: "follow up".into(),
                attachments: vec![d("a1")]
            }
        );
        assert!(h.take_drafts("dest").is_empty());
    }

    #[test]
    fn evicted_source_remaps_heap_drafts() {
        let mut h = HoistState::default();
        let mut heap = ChatHeapCache::default();
        heap.put(
            "host",
            &ChatHeapSnapshot {
                chat: [("host\tPA".to_string(), vec![body("hi")])].into_iter().collect(),
                input: [("host\tPA".to_string(), "follow up".to_string())].into_iter().collect(),
                attachments: [
                    ("host\tPA".to_string(), vec![d("a1")]),
                    ("host\tPB".to_string(), vec![d("a2")]),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        );
        h.migrate_evicted_drafts(&mut heap, "host", "dest", Some(&remap(&[("PA", "PA2")])));
        let drafts = h.take_drafts("dest");
        assert_eq!(drafts["dest\tPA2"].input, "follow up");
        assert_eq!(drafts["dest\tPA2"].attachments, vec![d("a1")]);
        assert_eq!(drafts["dest\tPB"].input, "");
        assert_eq!(drafts["dest\tPB"].attachments, vec![d("a2")]);
        let host = heap.get("host").expect("hit");
        assert!(host.input.is_empty() && host.attachments.is_empty());
        assert_eq!(host.chat["host\tPA"], vec![body("hi")]);
        assert_eq!(h.live_chat_key("host\tPA"), "dest\tPA2");

        let mut h = HoistState::default();
        let mut heap = ChatHeapCache::default();
        heap.put(
            "host",
            &ChatHeapSnapshot {
                chat: [("host\tPA".to_string(), vec![body("hi")])].into_iter().collect(),
                input: [("host\tPA".to_string(), "stale".to_string())].into_iter().collect(),
                ..Default::default()
            },
        );
        h.note_live_source("host");
        h.migrate_evicted_drafts(&mut heap, "host", "dest", Some(&remap(&[("PA", "PA2")])));
        h.drop_live_source("host");
        assert!(h.take_drafts("dest").is_empty());
        assert_eq!(heap.get("host").expect("hit").input["host\tPA"], "stale");
    }

    #[test]
    fn pending_and_sending_transfers() {
        let mut h = HoistState::default();
        h.stash_pending("dest\tPA", &[p("q1")], false);
        h.stash_pending("dest\tPA", &[p("q2")], true);
        assert!(h.take_pending("host").is_empty());
        assert_eq!(
            h.take_pending("dest")["dest\tPA"],
            HoistPending {
                items: vec![p("q1"), p("q2")],
                hold: true
            }
        );
        h.stash_sending("dest\tPA", 2);
        h.stash_sending("dest\tPA", 1);
        h.stash_pending("dest\tPA", &[p("q1")], false);
        assert!(h.take_sending("host").is_empty());
        assert_eq!(h.take_sending("dest"), [("dest\tPA".to_string(), 2)].into_iter().collect());
        assert!(h.take_sending("dest").is_empty());
        assert_eq!(h.take_pending("dest")["dest\tPA"].items, vec![p("q1")]);
        h.stash_sending("dest\tPA", 3);
        h.clear_sending("dest\tPA", 2);
        assert_eq!(h.take_sending("dest"), [("dest\tPA".to_string(), 3)].into_iter().collect());
        h.stash_sending("dest\tPA", 3);
        h.clear_sending("dest\tPA", 3);
        assert!(h.take_sending("dest").is_empty());
    }

    fn watch(h: &mut HoistState, books: SendBooks) -> (Rc<RefCell<SendBooks>>, Rc<RefCell<Vec<String>>>, u64) {
        let books = Rc::new(RefCell::new(books));
        let flushed = Rc::new(RefCell::new(Vec::new()));
        let (b, f) = (books.clone(), flushed.clone());
        let id = h.on_send_complete(move |key, gen_| {
            if !key.starts_with("dest\t") {
                return;
            }
            let mut bk = b.borrow_mut();
            let dest_gen = bk.send_gen.get(key).copied().unwrap_or(0);
            if !hoist_send_complete_applies(gen_, dest_gen, bk.hoist_send_gen.get(key).copied()) {
                return;
            }
            bk.sending.insert(key.to_string(), false);
            f.borrow_mut().push(key.to_string());
        });
        (books, flushed, id)
    }

    #[test]
    fn source_completion_releases_dest_after_absorb() {
        let mut h = HoistState::default();
        h.stash_sending("dest\tPA", 2);
        h.stash_pending("dest\tPA", &[p("q1")], false);
        h.note_alias("host\tPA", "dest\tPA");
        let inflight = h.take_sending("dest");
        let qs = h.take_pending("dest");
        assert_eq!(qs["dest\tPA"].items, vec![p("q1")]);
        let next = absorb_hoist_sending(SendBooks::default(), &inflight);
        let (books, flushed, id) = watch(&mut h, next);
        let (_, _, live) = h.live_chat_target("host", "PA");
        h.clear_sending("host\tPA", 2);
        h.clear_sending(&live, 2);
        assert_eq!(books.borrow().sending.get("dest\tPA"), Some(&false));
        assert_eq!(*flushed.borrow(), v(&["dest\tPA"]));
        h.off_send_complete(id);
    }

    #[test]
    fn source_completion_with_a_higher_dest_generation() {
        let mut h = HoistState::default();
        h.stash_sending("dest\tPA", 1);
        h.note_alias("host\tPA", "dest\tPA");
        let next = absorb_hoist_sending(
            SendBooks {
                send_gen: [("dest\tPA".to_string(), 3)].into_iter().collect(),
                ..Default::default()
            },
            &h.take_sending("dest"),
        );
        assert_eq!(next.send_gen["dest\tPA"], 3);
        assert_eq!(next.hoist_send_gen["dest\tPA"], 1);
        let (books, flushed, _) = watch(&mut h, next);
        let (_, _, live) = h.live_chat_target("host", "PA");
        h.clear_sending("host\tPA", 1);
        h.clear_sending(&live, 1);
        assert_eq!(books.borrow().sending.get("dest\tPA"), Some(&false));
        assert_eq!(*flushed.borrow(), v(&["dest\tPA"]));
    }

    #[test]
    fn source_completion_does_not_release_a_busy_dest() {
        for dest_gen in [3, 1] {
            let mut h = HoistState::default();
            h.stash_sending("dest\tPA", 1);
            let next = absorb_hoist_sending(
                SendBooks {
                    sending: [("dest\tPA".to_string(), true)].into_iter().collect(),
                    send_gen: [("dest\tPA".to_string(), dest_gen)].into_iter().collect(),
                    ..Default::default()
                },
                &h.take_sending("dest"),
            );
            assert!(!next.hoist_send_gen.contains_key("dest\tPA"));
            let (books, flushed, _) = watch(&mut h, next);
            h.clear_sending("dest\tPA", 1);
            assert_eq!(books.borrow().sending.get("dest\tPA"), Some(&true));
            assert!(flushed.borrow().is_empty());
        }
    }

    #[test]
    fn leftover_dest_gen_after_remap_still_releases() {
        let mut h = HoistState::default();
        h.stash_sending("dest\tPA", 1);
        let next = absorb_hoist_sending(
            SendBooks {
                sending: [("dest\tPA".to_string(), true)].into_iter().collect(),
                send_gen: [("dest\tPA".to_string(), 3)].into_iter().collect(),
                hoist_send_gen: [("dest\tPA".to_string(), 1)].into_iter().collect(),
            },
            &h.take_sending("dest"),
        );
        assert_eq!(next.hoist_send_gen["dest\tPA"], 1);
        let (books, flushed, _) = watch(&mut h, next);
        h.clear_sending("dest\tPA", 1);
        assert_eq!(books.borrow().sending.get("dest\tPA"), Some(&false));
        assert_eq!(*flushed.borrow(), v(&["dest\tPA"]));
        assert!(hoist_send_complete_applies(1, 3, Some(1)));
        assert!(!hoist_send_complete_applies(1, 4, None));
        assert!(!hoist_send_complete_applies(1, 3, None));
        assert!(!hoist_send_complete_applies(1, 1, None));
    }
}
