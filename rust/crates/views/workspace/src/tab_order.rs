//! `lib/chat-tab-reorder.ts`. `reorderByInsert`, `insertIndexFromPointerX` and
//! `restoreStripOrder` (and the 4px drag threshold) are the tab strip's
//! (`recur_ui::widgets::tab_strip`); this is the chat strip's saved-order rule.

use recur_domain::WorkspaceChatTab;

/// `applyChatTabOrder`: apply a saved strip order. Unknown and closed codes are skipped; Loop
/// still leads when it is open but missing from the order; other new tabs append.
pub fn apply_chat_tab_order(tabs: &[WorkspaceChatTab], order: &[String]) -> Vec<WorkspaceChatTab> {
    if order.is_empty() || tabs.len() < 2 {
        return tabs.to_vec();
    }
    let by_code = |code: &str| tabs.iter().find(|t| t.code == code);
    let mut out: Vec<WorkspaceChatTab> = Vec::with_capacity(tabs.len());
    let mut seen: Vec<&str> = Vec::new();
    if let Some(ma) = by_code("MA")
        && !order.iter().any(|c| c == "MA")
    {
        out.push(ma.clone());
        seen.push("MA");
    }
    for code in order {
        if seen.contains(&code.as_str()) {
            continue;
        }
        if let Some(t) = by_code(code) {
            out.push(t.clone());
            seen.push(code);
        }
    }
    for t in tabs {
        if !seen.contains(&t.code.as_str()) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_ui::widgets::tab_strip::{
        insert_index_from_pointer_x, reorder_by_insert, restore_strip_order,
    };
    use std::collections::BTreeMap;

    fn tab(code: &str) -> WorkspaceChatTab {
        WorkspaceChatTab {
            code: code.into(),
            ..Default::default()
        }
    }

    fn codes(tabs: &[WorkspaceChatTab]) -> Vec<String> {
        tabs.iter().map(|t| t.code.clone()).collect()
    }

    fn order(o: &[&str]) -> Vec<String> {
        o.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reorder_by_insert_moves_an_item_to_a_gap_in_the_remaining_list() {
        assert_eq!(reorder_by_insert(&["a", "b", "c"], 2, 0), ["c", "a", "b"]);
        assert_eq!(reorder_by_insert(&["a", "b", "c"], 0, 2), ["b", "c", "a"]);
        assert_eq!(reorder_by_insert(&["a", "b", "c"], 1, 1), ["a", "b", "c"]);
    }

    #[test]
    fn reorder_by_insert_clamps_a_past_the_end_insert_and_ignores_a_bad_from_index() {
        assert_eq!(reorder_by_insert(&["a", "b"], 0, 9), ["b", "a"]);
        assert_eq!(reorder_by_insert(&["a", "b"], -1, 0), ["a", "b"]);
        assert_eq!(reorder_by_insert(&["a", "b"], 2, 0), ["a", "b"]);
    }

    #[test]
    fn insert_index_picks_the_first_midpoint_to_the_right_of_the_pointer() {
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 5.), 0);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 30.), 2);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 29.), 1);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 80.), 3);
    }

    #[test]
    fn insert_index_appends_when_there_are_no_other_chips() {
        assert_eq!(insert_index_from_pointer_x(&[], 40.), 0);
    }

    #[test]
    fn restore_strip_order_restores_snapshot_order_for_keys_that_are_still_live() {
        assert_eq!(
            restore_strip_order(&["a", "c", "b"], &["c", "a", "b"], |s| *s),
            ["c", "a", "b"]
        );
    }

    #[test]
    fn restore_strip_order_drops_snapshot_keys_that_left_and_appends_keys_that_arrived() {
        assert_eq!(
            restore_strip_order(&["a", "d"], &["c", "a", "b"], |s| *s),
            ["a", "d"]
        );
        assert_eq!(
            restore_strip_order(&["c", "a", "b", "d"], &["c", "a", "b"], |s| *s),
            ["c", "a", "b", "d"]
        );
    }

    #[test]
    fn captured_owner_restore_does_not_write_into_a_different_workspace_list() {
        let mut by_ws: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        by_ws.insert("W1", vec!["PA", "MA", "PA2"]);
        by_ws.insert("W2", vec!["MA", "PA"]);
        let owner = "W1";
        let restored = restore_strip_order(&by_ws[owner], &["MA", "PA", "PA2"], |s| *s);
        by_ws.insert(owner, restored);
        assert_eq!(by_ws["W1"], ["MA", "PA", "PA2"]);
        assert_eq!(by_ws["W2"], ["MA", "PA"]);
    }

    fn def() -> Vec<WorkspaceChatTab> {
        vec![tab("MA"), tab("PA"), tab("PA2"), tab("PA3")]
    }

    #[test]
    fn empty_order_keeps_default() {
        assert_eq!(
            codes(&apply_chat_tab_order(&def(), &[])),
            ["MA", "PA", "PA2", "PA3"]
        );
    }

    #[test]
    fn single_tab_ignores_order() {
        assert_eq!(
            codes(&apply_chat_tab_order(&[tab("PA")], &order(&["PA2", "PA"]))),
            ["PA"]
        );
    }

    #[test]
    fn saved_permutation() {
        assert_eq!(
            codes(&apply_chat_tab_order(
                &def(),
                &order(&["PA2", "MA", "PA3", "PA"])
            )),
            ["PA2", "MA", "PA3", "PA"]
        );
    }

    #[test]
    fn closed_codes_are_skipped() {
        assert_eq!(
            codes(&apply_chat_tab_order(
                &[tab("MA"), tab("PA"), tab("PA3")],
                &order(&["PA2", "PA3", "MA", "PA"])
            )),
            ["PA3", "MA", "PA"]
        );
    }

    #[test]
    fn new_pair_tab_appends() {
        assert_eq!(
            codes(&apply_chat_tab_order(&def(), &order(&["PA2", "MA", "PA"]))),
            ["PA2", "MA", "PA", "PA3"]
        );
    }

    #[test]
    fn new_loop_still_leads() {
        assert_eq!(
            codes(&apply_chat_tab_order(&def(), &order(&["PA2", "PA", "PA3"]))),
            ["MA", "PA2", "PA", "PA3"]
        );
    }

    #[test]
    fn duplicate_order_codes_are_ignored() {
        assert_eq!(
            codes(&apply_chat_tab_order(
                &[tab("PA"), tab("PA2")],
                &order(&["PA2", "PA2", "PA"])
            )),
            ["PA2", "PA"]
        );
    }
}
