//! Frecency for the Run Command palette (`lib/command-usage.ts`): most recently run × most
//! often, persisted under `recur.commandUsage`. The Observatory records bundle commands run
//! from their key equivalents.

use std::collections::BTreeMap;

use gpui_kit::{App, Entity};
use recur_view_core::{AppState, LocalStorage};
use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "recur.commandUsage";
const MAX_TRACKED: usize = 80;
/// `COMMAND_USAGE_RECENT_SIZE`.
pub const RECENT_SECTION_SIZE: usize = 8;
/// Half-life of the recency decay: a week.
const HALF_LIFE_MS: f64 = 7. * 24. * 60. * 60. * 1000.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandUsage {
    pub count: u64,
    /// Epoch ms.
    pub last_used: i64,
}

type UsageMap = BTreeMap<String, CommandUsage>;

fn load(storage: &LocalStorage) -> UsageMap {
    storage
        .get(STORAGE_KEY)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `frecencyScore`: count × 0.5^(age / half-life).
pub fn frecency_score(u: Option<&CommandUsage>, now: i64) -> f64 {
    let Some(u) = u.filter(|u| u.count > 0) else {
        return 0.;
    };
    let age = (now - u.last_used).max(0) as f64;
    u.count as f64 * 0.5f64.powf(age / HALF_LIFE_MS)
}

/// One invocation, keeping the `MAX_TRACKED` best scores.
pub fn record_in(storage: &mut LocalStorage, id: &str, now: i64) {
    if id.is_empty() {
        return;
    }
    let mut map = load(storage);
    let prev = map.get(id).copied();
    map.insert(
        id.to_string(),
        CommandUsage {
            count: prev.map_or(0, |p| p.count) + 1,
            last_used: now,
        },
    );
    if map.len() > MAX_TRACKED {
        let mut ranked: Vec<(String, f64)> = map
            .iter()
            .map(|(k, u)| (k.clone(), frecency_score(Some(u), now)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let keep: std::collections::HashSet<String> = ranked
            .into_iter()
            .take(MAX_TRACKED)
            .map(|(k, _)| k)
            .collect();
        map.retain(|k, _| keep.contains(k));
    }
    if let Ok(raw) = serde_json::to_string(&map) {
        storage.set(STORAGE_KEY, raw);
    }
}

/// `recordCommandUsage`.
pub fn record_command_usage(state: &Entity<AppState>, id: &str, cx: &mut App) {
    let now = now_ms();
    state.update(cx, |s, _| record_in(s.storage_mut(), id, now));
}

/// `rankedCommandIds`: the last-used first, then by frecency.
pub fn ranked_command_ids(storage: &LocalStorage, limit: usize, now: i64) -> Vec<String> {
    let map = load(storage);
    let ids: Vec<&String> = map
        .keys()
        .filter(|id| frecency_score(map.get(*id), now) > 0.)
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let mut last: Option<&String> = None;
    let mut last_ts = -1i64;
    for id in &ids {
        let ts = map[*id].last_used;
        if ts > last_ts {
            last_ts = ts;
            last = Some(id);
        }
    }
    let mut rest: Vec<(&String, f64)> = ids
        .iter()
        .filter(|id| Some(**id) != last)
        .map(|id| (*id, frecency_score(map.get(*id), now)))
        .collect();
    rest.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| map[b.0].last_used.cmp(&map[a.0].last_used))
    });
    last.into_iter()
        .chain(rest.into_iter().map(|(id, _)| id))
        .take(limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn records_counts_and_ranks_last_used_first() {
        let mut s = LocalStorage::in_memory();
        let t0 = 1_700_000_000_000;
        record_in(&mut s, "a", t0);
        record_in(&mut s, "a", t0 + 1);
        record_in(&mut s, "a", t0 + 2);
        record_in(&mut s, "b", t0 + 3);
        record_in(&mut s, "c", t0 + 4);
        record_in(&mut s, "c", t0 + 5);
        let ranked = ranked_command_ids(&s, RECENT_SECTION_SIZE, t0 + 10);
        assert_eq!(ranked, vec!["c", "a", "b"]);
        assert_eq!(load(&s)["a"].count, 3);
    }

    #[test]
    fn frecency_halves_each_week() {
        let u = CommandUsage {
            count: 4,
            last_used: 0,
        };
        assert!((frecency_score(Some(&u), 7 * DAY) - 2.).abs() < 1e-9);
        assert_eq!(frecency_score(None, 0), 0.);
    }

    #[test]
    fn caps_tracked_commands() {
        let mut s = LocalStorage::in_memory();
        for i in 0..(MAX_TRACKED + 5) {
            record_in(&mut s, &format!("cmd{i}"), 1_000 + i as i64);
        }
        assert_eq!(load(&s).len(), MAX_TRACKED);
        assert!(load(&s).contains_key(&format!("cmd{}", MAX_TRACKED + 4)));
    }
}
