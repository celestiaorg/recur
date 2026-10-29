//! An insertion-order LRU with a resident-byte budget. The newest
//! entry is always kept even when it alone exceeds the budget.

use std::collections::VecDeque;

/// JS strings are UTF-16; the header is a budgeting fudge, as in the TypeScript.
const STR_HDR: usize = 24;
pub const OBJ_HDR: usize = 32;

/// Per-kind heap budget for the chat and observatory parks.
pub const WARM_CACHE_MAX_BYTES: usize = 128 * 1024 * 1024;
/// Tiny chrome (tab strip, transcript UI).
pub const CHROME_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// `stringBytes`: a JS string's budgeted size.
pub fn string_bytes(s: &str) -> usize {
    STR_HDR + s.encode_utf16().count() * 2
}

/// The budgeted size of a JSON-like object with `keys` (each a string) and the given value
/// bytes (`estimateBytes` over a plain object).
pub fn object_bytes<'a>(fields: impl IntoIterator<Item = (&'a str, usize)>) -> usize {
    OBJ_HDR
        + fields
            .into_iter()
            .map(|(k, v)| string_bytes(k) + v)
            .sum::<usize>()
}

/// A number or boolean.
pub const SCALAR_BYTES: usize = 8;

/// An array of values.
pub fn array_bytes(items: impl IntoIterator<Item = usize>) -> usize {
    let mut n = OBJ_HDR;
    for b in items {
        n += 8 + b;
    }
    n
}

struct Slot<V> {
    key: String,
    value: V,
    bytes: usize,
}

pub struct ByteLru<V> {
    slots: VecDeque<Slot<V>>,
    total: usize,
    max_bytes: usize,
    entry_max_bytes: usize,
    size_of: fn(&V) -> usize,
}

impl<V> ByteLru<V> {
    pub fn new(max_bytes: usize, size_of: fn(&V) -> usize) -> Self {
        ByteLru {
            slots: VecDeque::new(),
            total: 0,
            max_bytes,
            entry_max_bytes: 0,
            size_of,
        }
    }

    /// Skip inserts (and drop the previous value) when one entry exceeds `n` bytes.
    pub fn with_entry_max(mut self, n: usize) -> Self {
        self.entry_max_bytes = n;
        self
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn set_max_bytes(&mut self, n: usize) {
        self.max_bytes = n;
        self.evict("");
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.total
    }

    pub fn has(&self, key: &str) -> bool {
        self.slots.iter().any(|s| s.key == key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.key.as_str())
    }

    /// A read that does not promote.
    pub fn peek(&self, key: &str) -> Option<&V> {
        self.slots.iter().find(|s| s.key == key).map(|s| &s.value)
    }

    pub fn peek_mut(&mut self, key: &str) -> Option<&mut V> {
        self.slots
            .iter_mut()
            .find(|s| s.key == key)
            .map(|s| &mut s.value)
    }

    /// An LRU-promoting read.
    pub fn get(&mut self, key: &str) -> Option<&V> {
        let i = self.slots.iter().position(|s| s.key == key)?;
        let slot = self.slots.remove(i)?;
        self.slots.push_back(slot);
        self.slots.back().map(|s| &s.value)
    }

    /// Inserts `value` as newest; returns the entries dropped to fit the budget (not a same-key
    /// replace). The newest is never dropped.
    pub fn set(&mut self, key: &str, value: V) -> Vec<(String, V)> {
        if key.is_empty() {
            return Vec::new();
        }
        let bytes = (self.size_of)(&value);
        self.drop_key(key);
        if self.entry_max_bytes > 0 && bytes > self.entry_max_bytes {
            return Vec::new();
        }
        self.slots.push_back(Slot {
            key: key.to_string(),
            value,
            bytes,
        });
        self.total += bytes;
        self.evict(key)
    }

    /// Removes and returns without promoting anything.
    pub fn take(&mut self, key: &str) -> Option<V> {
        self.drop_key(key).map(|(_, v)| v)
    }

    pub fn delete(&mut self, key: &str) -> bool {
        self.drop_key(key).is_some()
    }

    /// Re-measures a value mutated in place without changing LRU order.
    pub fn refresh(&mut self, key: &str) {
        let size_of = self.size_of;
        if let Some(slot) = self.slots.iter_mut().find(|s| s.key == key) {
            self.total = self.total.saturating_sub(slot.bytes);
            slot.bytes = size_of(&slot.value);
            self.total += slot.bytes;
        }
    }

    /// Drops every entry `keep` rejects, returning the dropped values.
    pub fn prune(&mut self, mut keep: impl FnMut(&str, &V) -> bool) -> Vec<V> {
        let mut dropped = Vec::new();
        let mut kept = VecDeque::with_capacity(self.slots.len());
        for slot in self.slots.drain(..) {
            if keep(&slot.key, &slot.value) {
                kept.push_back(slot);
            } else {
                self.total = self.total.saturating_sub(slot.bytes);
                dropped.push(slot.value);
            }
        }
        self.slots = kept;
        dropped
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.total = 0;
    }

    fn drop_key(&mut self, key: &str) -> Option<(String, V)> {
        let i = self.slots.iter().position(|s| s.key == key)?;
        let slot = self.slots.remove(i)?;
        self.total = self.total.saturating_sub(slot.bytes);
        Some((slot.key, slot.value))
    }

    fn evict(&mut self, keep_key: &str) -> Vec<(String, V)> {
        let mut dropped = Vec::new();
        if self.max_bytes == 0 {
            return dropped;
        }
        while self.total > self.max_bytes && self.slots.len() > 1 {
            if self.slots.front().is_some_and(|s| s.key == keep_key) {
                break;
            }
            let Some(slot) = self.slots.pop_front() else {
                break;
            };
            self.total = self.total.saturating_sub(slot.bytes);
            dropped.push((slot.key, slot.value));
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::ptr_arg)]
    fn len(s: &String) -> usize {
        s.len()
    }

    #[test]
    fn string_and_object_estimates() {
        assert_eq!(string_bytes("ab"), 24 + 4);
        assert!(object_bytes([("a", string_bytes("x"))]) > string_bytes("a") + string_bytes("x"));
    }

    #[test]
    fn promotes_on_get_and_evicts_oldest() {
        let mut lru = ByteLru::new(6, len);
        for (k, v) in [("a", "aa"), ("b", "bb"), ("c", "cc"), ("d", "dd")] {
            lru.set(k, v.to_string());
        }
        assert!(lru.peek("a").is_none());
        assert_eq!(lru.get("b").map(String::as_str), Some("bb"));
        lru.set("e", "ee".into());
        assert!(lru.peek("c").is_none());
        assert_eq!(lru.peek("b").map(String::as_str), Some("bb"));
        assert_eq!(lru.peek("d").map(String::as_str), Some("dd"));
    }

    #[test]
    fn keeps_a_single_oversized_newest_entry() {
        let mut lru = ByteLru::new(4, len);
        lru.set("a", "aa".into());
        lru.set("big", "zzzzzzzz".into());
        assert!(lru.peek("a").is_none());
        assert_eq!(lru.peek("big").map(String::as_str), Some("zzzzzzzz"));
        assert_eq!(lru.bytes(), 8);
    }

    #[test]
    fn skips_inserts_over_entry_max() {
        let mut lru = ByteLru::new(100, len).with_entry_max(4);
        lru.set("k", "ab".into());
        assert!(lru.set("k", "too-big".into()).is_empty());
        assert!(lru.peek("k").is_none());
    }

    #[test]
    fn take_transfers_without_promoting() {
        let mut lru = ByteLru::new(5, len);
        lru.set("a", "aa".into());
        lru.set("b", "bb".into());
        assert_eq!(lru.take("a").as_deref(), Some("aa"));
        assert!(lru.take("a").is_none());
        lru.set("c", "cc".into());
        lru.set("d", "dd".into());
        assert!(lru.peek("b").is_none());
        assert_eq!(lru.peek("c").map(String::as_str), Some("cc"));
        assert_eq!(lru.peek("d").map(String::as_str), Some("dd"));
    }

    #[test]
    fn refresh_remeasures_in_place() {
        let mut lru = ByteLru::new(100, len);
        lru.set("a", "aa".into());
        lru.set("b", "bb".into());
        assert_eq!(lru.bytes(), 4);
        if let Some(v) = lru.peek_mut("a") {
            v.clear();
        }
        lru.refresh("a");
        assert_eq!(lru.bytes(), 2);
        assert_eq!(lru.keys().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn prune_and_shrink() {
        let mut lru = ByteLru::new(100, len);
        lru.set("keep", "aa".into());
        lru.set("drop", "bb".into());
        assert_eq!(lru.prune(|k, _| k == "keep"), vec!["bb".to_string()]);
        assert!(lru.peek("drop").is_none());
        lru.set("two", "cc".into());
        lru.set_max_bytes(2);
        assert!(lru.peek("keep").is_none());
        assert_eq!(lru.peek("two").map(String::as_str), Some("cc"));
    }
}
