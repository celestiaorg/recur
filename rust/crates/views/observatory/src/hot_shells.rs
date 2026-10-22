//! The live workbench budget (`lib/hot-shells.ts`): the visible shell plus the previous one
//! stay mounted; A→B→A is then a reuse, not a rebuild.

use std::collections::HashSet;

use recur_domain::Workspace;

/// The measured cap. Raising the live cap made hot switches slower.
pub const HOT_SHELL_MAX: usize = 2;

/// Newest-first LRU: `id` becomes the head; entries missing from `live` drop.
pub fn touch_hot_shells(
    hot: &[String],
    id: &str,
    max: Option<usize>,
    live: Option<&HashSet<String>>,
) -> Vec<String> {
    let max = max.unwrap_or(HOT_SHELL_MAX);
    let mut next: Vec<String> = Vec::new();
    let push = |x: &str, next: &mut Vec<String>| {
        if x.is_empty() || next.iter().any(|n| n == x) {
            return;
        }
        if live.is_some_and(|l| !l.contains(x)) {
            return;
        }
        next.push(x.to_string());
    };
    push(id, &mut next);
    for x in hot {
        if next.len() >= max {
            break;
        }
        push(x, &mut next);
    }
    next
}

pub fn hot_shells_equal(a: &[String], b: &[String]) -> bool {
    a == b
}

/// Mounted workbench ids: archived rows stay in the store but keep no shell.
pub fn live_workspace_ids(workspaces: &[Workspace]) -> HashSet<String> {
    workspaces
        .iter()
        .filter(|w| !w.id.is_empty() && !w.archived)
        .map(|w| w.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn caps_at_two_and_keeps_the_previous_id() {
        assert_eq!(HOT_SHELL_MAX, 2);
        assert_eq!(touch_hot_shells(&[], "a", None, None), v(&["a"]));
        assert_eq!(
            touch_hot_shells(&v(&["a"]), "b", None, None),
            v(&["b", "a"])
        );
        assert_eq!(
            touch_hot_shells(&v(&["b", "a"]), "c", None, None),
            v(&["c", "b"])
        );
    }

    #[test]
    fn moving_back_to_a_parked_id_is_a_reorder() {
        assert_eq!(
            touch_hot_shells(&v(&["b", "a"]), "a", None, None),
            v(&["a", "b"])
        );
    }

    #[test]
    fn empty_id_does_not_shift_the_lru() {
        assert_eq!(
            touch_hot_shells(&v(&["b", "a"]), "", None, None),
            v(&["b", "a"])
        );
    }

    #[test]
    fn drops_ids_that_are_no_longer_live() {
        assert_eq!(
            touch_hot_shells(&v(&["b", "a"]), "b", None, Some(&set(&["b"]))),
            v(&["b"])
        );
        assert_eq!(
            touch_hot_shells(&v(&["gone"]), "a", None, Some(&set(&["a"]))),
            v(&["a"])
        );
    }

    #[test]
    fn ignores_a_select_of_a_workspace_that_is_not_live() {
        assert_eq!(
            touch_hot_shells(&v(&["a"]), "deleted", None, Some(&set(&["a"]))),
            v(&["a"])
        );
    }

    #[test]
    fn live_ids_drop_archived_and_empty() {
        let w = |id: &str, archived: bool| Workspace {
            id: id.into(),
            archived,
            ..Default::default()
        };
        assert_eq!(
            live_workspace_ids(&[w("a", false), w("b", true), w("", false), w("c", false)]),
            set(&["a", "c"])
        );
    }

    #[test]
    fn equal_compares_by_order() {
        assert!(hot_shells_equal(&v(&["a", "b"]), &v(&["a", "b"])));
        assert!(!hot_shells_equal(&v(&["a", "b"]), &v(&["b", "a"])));
        assert!(!hot_shells_equal(&v(&["a"]), &v(&["a", "b"])));
    }
}
