//! In-memory transcript chrome per chat key (workspace + agent code), as
//! `lib/caches/chat-transcript-ui.ts`: one chat view serves every tab, so a swap saves the
//! leaving chat's expanded groups, rows, finding accordions, stub heights and scroll, and a
//! return restores them. The cache is a GPUI global ([`TranscriptUiCache`]).

use std::collections::BTreeMap;

use gpui_kit::{App, Global};

use crate::byte_lru::{ByteLru, CHROME_CACHE_MAX_BYTES, SCALAR_BYTES, object_bytes, string_bytes};

/// Distance from the tail (px) that counts as stuck to the bottom for auto-follow.
pub const NEAR_BOTTOM_PX: f32 = 24.;

/// `scrollerNearBottom`.
pub fn scroller_near_bottom(scroll_height: f32, scroll_top: f32, client_height: f32) -> bool {
    scroll_height - scroll_top - client_height < NEAR_BOTTOM_PX
}

/// `nextNearBottom`: stick-to-bottom after a scroll. Fast streaming grows the canvas between a
/// pin and the event, so a distance past the slack must not unpin; an upward move leaves the
/// tail even inside the slack.
pub fn next_near_bottom(
    was_near: bool,
    prev_top: f32,
    scroll_height: f32,
    scroll_top: f32,
    client_height: f32,
) -> bool {
    if scroll_top < prev_top - 1. {
        return false;
    }
    if scroller_near_bottom(scroll_height, scroll_top, client_height) {
        return true;
    }
    was_near
}

/// One chat's saved chrome (`ChatTranscriptUIState`).
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptUiState {
    /// Step groups by their first row index.
    pub open_groups: BTreeMap<usize, bool>,
    /// Rows by row key (`"7"`, `"7:2"` for nested children).
    pub open_rows: BTreeMap<String, bool>,
    pub finding_accord_open: BTreeMap<String, bool>,
    pub scroll_top: f32,
    pub near_bottom: bool,
    /// Transcript row count at capture: the expansion maps are index-based.
    pub row_count: usize,
    /// Row-kind signature; prefix-stable while the transcript only appends.
    pub fingerprint: String,
    /// Measured block heights so a remount can stub at the same offsets.
    pub block_heights: BTreeMap<String, f32>,
}

impl Default for TranscriptUiState {
    fn default() -> Self {
        empty_transcript_ui()
    }
}

/// `emptyTranscriptUI`.
pub fn empty_transcript_ui() -> TranscriptUiState {
    TranscriptUiState {
        open_groups: BTreeMap::new(),
        open_rows: BTreeMap::new(),
        finding_accord_open: BTreeMap::new(),
        scroll_top: 0.,
        near_bottom: true,
        row_count: 0,
        fingerprint: String::new(),
        block_heights: BTreeMap::new(),
    }
}

/// A `Record<string, number | boolean>`.
fn bool_map_bytes(keys: impl IntoIterator<Item = String>) -> usize {
    crate::byte_lru::OBJ_HDR
        + keys
            .into_iter()
            .map(|k| string_bytes(&k) + SCALAR_BYTES)
            .sum::<usize>()
}

/// `estimateBytes` over the state's JSON shape.
fn state_bytes(s: &TranscriptUiState) -> usize {
    let groups = bool_map_bytes(s.open_groups.keys().map(|k| k.to_string()));
    let rows = bool_map_bytes(s.open_rows.keys().cloned());
    let accord = bool_map_bytes(s.finding_accord_open.keys().cloned());
    let heights = bool_map_bytes(s.block_heights.keys().cloned());
    object_bytes([
        ("openGroups", groups),
        ("openRows", rows),
        ("findingAccordOpen", accord),
        ("scrollTop", SCALAR_BYTES),
        ("nearBottom", SCALAR_BYTES),
        ("rowCount", SCALAR_BYTES),
        ("fingerprint", string_bytes(&s.fingerprint)),
        ("blockHeights", heights),
    ])
}

/// The process-wide chrome cache plus tombstones for chats closed while their view tears down.
pub struct TranscriptUiCache {
    cache: ByteLru<TranscriptUiState>,
    closed: ByteLru<()>,
}

impl Global for TranscriptUiCache {}

impl Default for TranscriptUiCache {
    fn default() -> Self {
        TranscriptUiCache {
            cache: ByteLru::new(CHROME_CACHE_MAX_BYTES, state_bytes),
            closed: ByteLru::new(CHROME_CACHE_MAX_BYTES, |_| 64),
        }
    }
}

impl TranscriptUiCache {
    pub fn global(cx: &mut App) -> &mut TranscriptUiCache {
        cx.default_global::<TranscriptUiCache>()
    }

    /// `saveTranscriptUI`.
    pub fn save(&mut self, key: &str, state: TranscriptUiState) {
        if key.is_empty() || self.closed.has(key) {
            return;
        }
        self.cache.set(key, state);
    }

    /// `saveTranscriptUIScroll`: a scroll-only write over the cached maps.
    pub fn save_scroll(&mut self, key: &str, scroll_top: f32, near_bottom: bool) {
        if key.is_empty() || self.closed.has(key) {
            return;
        }
        let Some(prev) = self.cache.peek(key).cloned() else {
            return;
        };
        self.cache.set(
            key,
            TranscriptUiState {
                scroll_top,
                near_bottom,
                ..prev
            },
        );
    }

    /// `loadTranscriptUI`.
    pub fn load(&self, key: &str) -> Option<TranscriptUiState> {
        if key.is_empty() {
            return None;
        }
        self.cache.peek(key).cloned()
    }

    /// `claimTranscriptUI`: re-open a dropped key (a new tab reused the agent code).
    pub fn claim(&mut self, key: &str) -> Option<TranscriptUiState> {
        if key.is_empty() {
            return None;
        }
        self.closed.delete(key);
        self.load(key)
    }

    /// `dropTranscriptUI`: forget a closed chat and tombstone it against late saves.
    pub fn drop_key(&mut self, key: &str) {
        if key.is_empty() {
            return;
        }
        self.cache.delete(key);
        self.closed.set(key, ());
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.closed.clear();
        self.cache.set_max_bytes(CHROME_CACHE_MAX_BYTES);
        self.closed.set_max_bytes(CHROME_CACHE_MAX_BYTES);
    }

    pub fn set_max_bytes_for_tests(&mut self, n: usize) {
        self.cache.set_max_bytes(n);
        self.closed.set_max_bytes(n);
    }
}

/// `restoreBlockHeights`: saved stub heights into a fresh per-chat map.
pub fn restore_block_heights(into: &mut BTreeMap<String, f32>, saved: &BTreeMap<String, f32>) {
    for (k, h) in saved {
        if *h > 0. {
            into.insert(k.clone(), *h);
        }
    }
}

/// The fields [`transcript_fingerprint`] reads from a row.
pub struct FingerprintRow<'a> {
    pub tool: &'a str,
    pub thinking: bool,
    pub compact: bool,
    pub note: bool,
    pub fallback: bool,
    pub me: bool,
}

impl<'a> From<&'a recur_engine::view::chat::AgentMsg> for FingerprintRow<'a> {
    fn from(m: &'a recur_engine::view::chat::AgentMsg) -> Self {
        FingerprintRow {
            tool: &m.tool,
            thinking: m.thinking,
            compact: m.compact,
            note: m.note,
            fallback: m.fallback,
            me: m.me,
        }
    }
}

/// `transcriptFingerprint`: a row-kind signature; `limit` fingerprints only a prefix.
pub fn transcript_fingerprint<'a>(
    rows: impl IntoIterator<Item = FingerprintRow<'a>>,
    limit: Option<usize>,
) -> String {
    let rows: Vec<FingerprintRow<'a>> = rows.into_iter().collect();
    let end = limit.map_or(rows.len(), |l| l.min(rows.len()));
    rows[..end]
        .iter()
        .map(|m| {
            if !m.tool.is_empty() {
                format!("t:{}", m.tool)
            } else if m.thinking {
                "th".into()
            } else if m.compact {
                "c".into()
            } else if m.note {
                "n".into()
            } else if m.fallback {
                "fb".into()
            } else if m.me {
                "u".into()
            } else {
                "a".into()
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// `expansionStillValid`: index-based maps only fit the transcript they were captured against
/// or an append-only continuation of it.
pub fn expansion_still_valid(
    saved_row_count: usize,
    saved_fingerprint: &str,
    fingerprint: &str,
    row_count: usize,
) -> bool {
    if row_count == 0 {
        return true;
    }
    if saved_fingerprint.is_empty() && saved_row_count == 0 {
        return true;
    }
    if row_count < saved_row_count {
        return false;
    }
    if row_count == saved_row_count {
        return fingerprint == saved_fingerprint;
    }
    fingerprint == saved_fingerprint || fingerprint.starts_with(&format!("{saved_fingerprint}|"))
}

/// Whether a pending scroll restore should apply, wait for layout, or be dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollRestoreGate {
    Wait,
    Apply,
    Abandon,
}

/// `scrollRestoreReady`.
pub fn scroll_restore_ready(
    saved_scroll_top: f32,
    saved_near_bottom: bool,
    saved_row_count: usize,
    client_height: f32,
    scroll_height: f32,
    row_count: usize,
) -> ScrollRestoreGate {
    if client_height <= 0. {
        return ScrollRestoreGate::Wait;
    }
    if saved_near_bottom || saved_scroll_top <= 0. {
        return ScrollRestoreGate::Apply;
    }
    let max_scroll = (scroll_height - client_height).max(0.);
    if max_scroll >= saved_scroll_top || max_scroll > 0. {
        return ScrollRestoreGate::Apply;
    }
    if row_count > 0 && (saved_row_count == 0 || row_count >= saved_row_count) {
        return ScrollRestoreGate::Abandon;
    }
    ScrollRestoreGate::Wait
}

/// `transcriptBlockKey`: a row's identity, prefixed with the chat so live rows of a new chat
/// never reuse the previous chat's instances.
pub fn transcript_block_key(reset_key: &str, local: &str) -> String {
    let k = if reset_key.is_empty() { "_" } else { reset_key };
    format!("{k}:{local}")
}

/// `transcriptSessionBlockLocal`: one specialist row and a packed group share this.
pub fn transcript_session_block_local(log: &recur_engine::view::chat::ChatLog) -> String {
    use recur_engine::view::chat::ChatLog;
    let id = match log {
        ChatLog::Sessions { id, sessions } => {
            if !id.is_empty() {
                id.clone()
            } else {
                sessions.first().map(|s| s.id.clone()).unwrap_or_default()
            }
        }
        ChatLog::Session(s) => {
            if !s.id.is_empty() {
                s.id.clone()
            } else {
                s.code.clone()
            }
        }
        ChatLog::Fact { .. } => String::new(),
    };
    format!("sess:{id}")
}

/// `transcriptStepBlockLocal`: tool-led groups key off the provider tool id; `start` mixes in
/// only without one.
pub fn transcript_step_block_local(
    start: usize,
    first_tool_id: &str,
    first_ts: Option<i64>,
    first_body: &str,
    live_last: bool,
) -> String {
    if live_last {
        return "s:live".into();
    }
    if !first_tool_id.is_empty() {
        return format!("s:{first_tool_id}");
    }
    let tail = match first_ts.filter(|t| *t != 0) {
        Some(ts) => ts.to_string(),
        None => {
            let b: String = js_prefix(first_body, 24);
            if b.is_empty() { "x".into() } else { b }
        }
    };
    format!("s:{start}:{tail}")
}

/// `body.slice(0, n)` in UTF-16 units, never splitting a surrogate pair.
pub fn js_prefix(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = c.len_utf16();
        if units + w > n {
            break;
        }
        units += w;
        out.push(c);
    }
    out
}

/// `uniqueBlockKeys`: suffix repeats so keys stay unique.
pub fn unique_block_keys(keys: Vec<String>) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    keys.into_iter()
        .map(|k| {
            let n = seen.entry(k.clone()).or_default();
            let out = if *n > 0 { format!("{k}#{n}") } else { k };
            *n += 1;
            out
        })
        .collect()
}

/// `transcriptMessageBlockLocal`: timestamp + body/tool id so a mid-list splice does not
/// remount every following row; the index is the last fallback.
pub fn transcript_message_block_local(
    index: usize,
    me: bool,
    ts: Option<i64>,
    tool_id: &str,
    body: &str,
) -> String {
    let ts = ts.filter(|t| *t != 0);
    if me {
        let b = js_prefix(body, 48);
        if ts.is_some() || !b.is_empty() {
            return format!("me:{}:{b}", ts.unwrap_or(0));
        }
        return format!("me:{index}");
    }
    let tail = if !tool_id.is_empty() {
        tool_id.to_string()
    } else {
        let b = js_prefix(body, 32);
        if b.is_empty() { index.to_string() } else { b }
    };
    format!("m:{}:{tail}", ts.unwrap_or(0))
}

/// The live chrome a [`snapshot_transcript_ui`] captures.
pub struct LiveChrome<'a> {
    pub open_groups: &'a BTreeMap<usize, bool>,
    pub open_rows: &'a BTreeMap<String, bool>,
    pub finding_accord_open: &'a BTreeMap<String, bool>,
    pub scroll_top: f32,
    pub near_bottom: bool,
    /// The view already shows another chat (or is detached).
    pub scroller_hidden: bool,
    pub row_count: usize,
    pub fingerprint: &'a str,
    pub block_heights: &'a BTreeMap<String, f32>,
}

/// `snapshotTranscriptUI`: with the scroller hidden, keep the previous expansion pair unless
/// the caller still has a matched count + fingerprint. Scroll comes from the caller.
pub fn snapshot_transcript_ui(
    live: &LiveChrome<'_>,
    previous: Option<&TranscriptUiState>,
) -> TranscriptUiState {
    let (row_count, fingerprint) =
        if !live.scroller_hidden || (live.row_count > 0 && !live.fingerprint.is_empty()) {
            (live.row_count, live.fingerprint.to_string())
        } else {
            (
                previous.map_or(0, |p| p.row_count),
                previous.map(|p| p.fingerprint.clone()).unwrap_or_default(),
            )
        };
    let block_heights = if !live.block_heights.is_empty() {
        live.block_heights.clone()
    } else if live.scroller_hidden {
        previous
            .map(|p| p.block_heights.clone())
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    TranscriptUiState {
        open_groups: live.open_groups.clone(),
        open_rows: live.open_rows.clone(),
        finding_accord_open: live.finding_accord_open.clone(),
        scroll_top: live.scroll_top,
        near_bottom: live.near_bottom,
        row_count,
        fingerprint,
        block_heights,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_engine::view::chat::{ChatLog, ChatLogSession};

    fn expanded() -> TranscriptUiState {
        TranscriptUiState {
            open_groups: [(2, true)].into_iter().collect(),
            open_rows: [("3".to_string(), true), ("4".to_string(), false)]
                .into_iter()
                .collect(),
            finding_accord_open: [("f1".to_string(), true)].into_iter().collect(),
            scroll_top: 480.,
            near_bottom: false,
            row_count: 12,
            fingerprint: "u|a|t:Bash".into(),
            block_heights: [("ws-a\tMA:m:1".to_string(), 240.)].into_iter().collect(),
        }
    }

    // chat-transcript-ui.test.ts
    #[test]
    fn live_row_keys_stay_distinct_across_chats() {
        assert_ne!(
            transcript_block_key("ws-a\tPA", "m:live-text"),
            transcript_block_key("ws-b\tPA", "m:live-text")
        );
        assert_eq!(
            transcript_block_key("ws-b\tPA", "s:live"),
            "ws-b\tPA:s:live"
        );
        assert_eq!(transcript_block_key("", "m:live-text"), "_:m:live-text");
    }

    #[test]
    fn unique_keys_suffix_repeats() {
        let keys = ["k:m:0:Hi", "k:me:1:x", "k:m:0:Hi", "k:m:0:Hi"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            unique_block_keys(keys),
            ["k:m:0:Hi", "k:me:1:x", "k:m:0:Hi#1", "k:m:0:Hi#2"]
        );
    }

    #[test]
    fn step_groups_sharing_a_timestamp_stay_unique() {
        let ts = 1786586607416;
        let a = transcript_step_block_local(19, "", Some(ts), "", false);
        let b = transcript_step_block_local(21, "", Some(ts), "", false);
        assert_eq!(a, format!("s:19:{ts}"));
        assert_eq!(b, format!("s:21:{ts}"));
        assert_ne!(a, b);
        assert_eq!(
            transcript_step_block_local(19, "", Some(ts), "", true),
            "s:live"
        );
        assert_eq!(
            transcript_step_block_local(12, "tool_abc123", Some(100), "", false),
            "s:tool_abc123"
        );
        assert_eq!(
            transcript_step_block_local(18, "tool_abc123", Some(100), "", false),
            "s:tool_abc123"
        );
    }

    #[test]
    fn session_keys_stay_stable_when_a_sibling_joins() {
        let one = ChatLogSession {
            id: "s-co#0".into(),
            code: "CO".into(),
            ..Default::default()
        };
        let group = ChatLog::Sessions {
            id: "s-co#0".into(),
            sessions: vec![
                one.clone(),
                ChatLogSession {
                    id: "s-rv#0".into(),
                    ..Default::default()
                },
            ],
        };
        let single = ChatLog::Session(one);
        assert_eq!(transcript_session_block_local(&single), "sess:s-co#0");
        assert_eq!(transcript_session_block_local(&group), "sess:s-co#0");
    }

    #[test]
    fn message_keys_stay_stable_when_a_row_is_spliced_above() {
        assert_eq!(
            transcript_message_block_local(3, true, Some(100), "", "hello"),
            "me:100:hello"
        );
        assert_eq!(
            transcript_message_block_local(5, true, Some(100), "", "hello"),
            "me:100:hello"
        );
        assert_eq!(
            transcript_message_block_local(4, false, Some(200), "", "ok"),
            "m:200:ok"
        );
        assert_eq!(
            transcript_message_block_local(0, true, None, "", ""),
            "me:0"
        );
        assert_eq!(
            transcript_message_block_local(2, false, None, "", ""),
            "m:0:2"
        );
    }

    #[test]
    fn round_trips_per_key() {
        let mut c = TranscriptUiCache::default();
        c.save("ws-a\tMA", expanded());
        c.save("ws-a\tPA", empty_transcript_ui());
        assert_eq!(c.load("ws-a\tMA"), Some(expanded()));
        assert_eq!(c.load("ws-a\tPA"), Some(empty_transcript_ui()));
        assert_eq!(c.load("ws-b\tMA"), None);
        c.save("", expanded());
        assert_eq!(c.load(""), None);
    }

    #[test]
    fn load_returns_a_copy() {
        let mut c = TranscriptUiCache::default();
        c.save("k", expanded());
        let mut a = c.load("k").expect("saved");
        a.open_rows.insert("3".into(), false);
        assert_eq!(c.load("k").expect("saved").open_rows.get("3"), Some(&true));
    }

    #[test]
    fn drop_tombstones_and_claim_reopens() {
        let mut c = TranscriptUiCache::default();
        c.save("a", expanded());
        c.save("b", empty_transcript_ui());
        c.drop_key("a");
        assert_eq!(c.load("a"), None);
        assert_eq!(c.load("b"), Some(empty_transcript_ui()));
        c.save(
            "a",
            TranscriptUiState {
                scroll_top: 1.,
                ..expanded()
            },
        );
        assert_eq!(c.load("a"), None);
        assert_eq!(c.claim("a"), None);
        c.save("a", expanded());
        assert_eq!(c.load("a"), Some(expanded()));
    }

    #[test]
    fn budget_holds_many_chats_and_evicts_oldest() {
        let mut c = TranscriptUiCache::default();
        for i in 0..80 {
            c.save(
                &format!("k{i}"),
                TranscriptUiState {
                    scroll_top: i as f32,
                    ..empty_transcript_ui()
                },
            );
        }
        assert_eq!(c.load("k0").map(|s| s.scroll_top), Some(0.));
        assert_eq!(c.load("k79").map(|s| s.scroll_top), Some(79.));
        let mut c = TranscriptUiCache::default();
        c.set_max_bytes_for_tests(1);
        for i in 0..3 {
            c.save(
                &format!("k{i}"),
                TranscriptUiState {
                    scroll_top: i as f32,
                    ..empty_transcript_ui()
                },
            );
        }
        assert_eq!(c.load("k0"), None);
        assert_eq!(c.load("k2").map(|s| s.scroll_top), Some(2.));
    }

    #[test]
    fn tombstones_share_the_budget() {
        let mut c = TranscriptUiCache::default();
        c.set_max_bytes_for_tests(1);
        for k in ["k0", "k1", "k2"] {
            c.save(k, expanded());
            c.drop_key(k);
        }
        c.save("k0", expanded());
        assert_eq!(c.load("k0"), Some(expanded()));
        c.save("k2", expanded());
        assert_eq!(c.load("k2"), None);
    }

    #[allow(clippy::too_many_arguments)]
    fn live<'a>(
        empty_b: &'a BTreeMap<usize, bool>,
        rows: &'a BTreeMap<String, bool>,
        accord: &'a BTreeMap<String, bool>,
        heights: &'a BTreeMap<String, f32>,
        hidden: bool,
        row_count: usize,
        fp: &'a str,
        near: bool,
        top: f32,
    ) -> LiveChrome<'a> {
        LiveChrome {
            open_groups: empty_b,
            open_rows: rows,
            finding_accord_open: accord,
            scroll_top: top,
            near_bottom: near,
            scroller_hidden: hidden,
            row_count,
            fingerprint: fp,
            block_heights: heights,
        }
    }

    #[test]
    fn snapshot_keeps_the_expansion_pair_when_unmeasured() {
        let groups: BTreeMap<usize, bool> = [(1, false)].into_iter().collect();
        let none = BTreeMap::new();
        let accord = BTreeMap::new();
        let heights = BTreeMap::new();
        let prev = expanded();
        let hidden = snapshot_transcript_ui(
            &live(&groups, &none, &accord, &heights, true, 0, "", true, 90.),
            Some(&prev),
        );
        assert_eq!(hidden.open_groups.get(&1), Some(&false));
        assert_eq!(hidden.scroll_top, 90.);
        assert!(hidden.near_bottom);
        assert_eq!(hidden.row_count, 12);
        assert_eq!(hidden.fingerprint, "u|a|t:Bash");
        let empty_g = BTreeMap::new();
        let leaving = snapshot_transcript_ui(
            &live(
                &empty_g, &none, &accord, &heights, true, 8, "u|a", false, 90.,
            ),
            Some(&prev),
        );
        assert_eq!(
            (leaving.row_count, leaving.fingerprint.as_str()),
            (8, "u|a")
        );
        let mismatched = snapshot_transcript_ui(
            &live(&empty_g, &none, &accord, &heights, true, 50, "", false, 90.),
            Some(&prev),
        );
        assert_eq!(mismatched.row_count, 12);
        assert_eq!(mismatched.fingerprint, "u|a|t:Bash");
        let rows: BTreeMap<String, bool> = [("8".to_string(), true)].into_iter().collect();
        let visible = snapshot_transcript_ui(
            &live(
                &empty_g, &rows, &accord, &heights, false, 3, "u|a", false, 12.,
            ),
            None,
        );
        assert_eq!(
            visible,
            TranscriptUiState {
                open_groups: BTreeMap::new(),
                open_rows: rows.clone(),
                finding_accord_open: BTreeMap::new(),
                scroll_top: 12.,
                near_bottom: false,
                row_count: 3,
                fingerprint: "u|a".into(),
                block_heights: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn snapshot_keeps_measured_heights_when_teardown_has_none() {
        let g = BTreeMap::new();
        let r = BTreeMap::new();
        let a = BTreeMap::new();
        let none = BTreeMap::new();
        let prev = expanded();
        let hidden = snapshot_transcript_ui(
            &live(&g, &r, &a, &none, true, 0, "", false, 90.),
            Some(&prev),
        );
        assert_eq!(hidden.block_heights, prev.block_heights);
        let h: BTreeMap<String, f32> = [("k:m:2".to_string(), 180.)].into_iter().collect();
        let live_s = snapshot_transcript_ui(&live(&g, &r, &a, &h, false, 0, "", false, 12.), None);
        assert_eq!(live_s.block_heights, h);
    }

    #[test]
    fn fingerprint_and_expansion() {
        let rows = [
            FingerprintRow {
                tool: "",
                thinking: false,
                compact: false,
                note: false,
                fallback: false,
                me: true,
            },
            FingerprintRow {
                tool: "",
                thinking: false,
                compact: false,
                note: false,
                fallback: false,
                me: false,
            },
            FingerprintRow {
                tool: "Bash",
                thinking: false,
                compact: false,
                note: false,
                fallback: false,
                me: false,
            },
        ];
        let fp = transcript_fingerprint(rows, None);
        assert_eq!(fp, "u|a|t:Bash");
        assert!(expansion_still_valid(3, &fp, "", 0));
        assert!(expansion_still_valid(3, &fp, &fp, 3));
        assert!(expansion_still_valid(3, &fp, &format!("{fp}|u"), 4));
        assert!(!expansion_still_valid(3, "u|a", &fp, 3));
        assert!(!expansion_still_valid(3, &fp, "u|a", 2));
        assert!(!expansion_still_valid(3, &fp, "u|a|t:Read", 3));
    }

    #[test]
    fn scroll_restore_gate() {
        use ScrollRestoreGate::*;
        assert_eq!(scroll_restore_ready(480., false, 20, 0., 0., 20), Wait);
        assert_eq!(scroll_restore_ready(0., true, 20, 400., 400., 0), Apply);
        assert_eq!(scroll_restore_ready(0., false, 20, 400., 400., 0), Apply);
        assert_eq!(scroll_restore_ready(480., false, 20, 400., 400., 0), Wait);
        assert_eq!(
            scroll_restore_ready(480., false, 20, 400., 400., 20),
            Abandon
        );
        assert_eq!(
            scroll_restore_ready(480., false, 20, 400., 1200., 20),
            Apply
        );
        assert_eq!(scroll_restore_ready(480., false, 20, 400., 500., 20), Apply);
        assert_eq!(scroll_restore_ready(900., false, 0, 400., 400., 8), Abandon);
        assert_eq!(scroll_restore_ready(900., false, 0, 400., 600., 8), Apply);
        assert_eq!(scroll_restore_ready(900., false, 0, 400., 400., 0), Wait);
    }

    #[test]
    fn near_bottom_rules() {
        assert!(!scroller_near_bottom(1000., 776., 200.));
        assert!(scroller_near_bottom(1000., 777., 200.));
        assert!(next_near_bottom(true, 700., 1000., 720., 200.));
        assert!(!next_near_bottom(true, 800., 1000., 720., 200.));
        assert!(!next_near_bottom(true, 800., 1000., 790., 200.));
        assert!(next_near_bottom(false, 700., 1000., 777., 200.));
    }

    #[test]
    fn scroll_only_saves() {
        let mut c = TranscriptUiCache::default();
        c.save("k", expanded());
        c.save_scroll("k", 12., true);
        assert_eq!(
            c.load("k"),
            Some(TranscriptUiState {
                scroll_top: 12.,
                near_bottom: true,
                ..expanded()
            })
        );
        let mut c = TranscriptUiCache::default();
        c.save_scroll("k", 12., false);
        assert_eq!(c.load("k"), None);
    }

    #[test]
    fn block_heights_restore() {
        let mut m = BTreeMap::new();
        let saved: BTreeMap<String, f32> = [("a", 240.), ("b", 0.), ("c", 56.)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        restore_block_heights(&mut m, &saved);
        assert_eq!(
            m.into_iter().collect::<Vec<_>>(),
            [("a".to_string(), 240.), ("c".to_string(), 56.)]
        );
    }
}
