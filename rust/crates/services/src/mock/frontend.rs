//! The two frontend helpers mock-bridge.ts imports: `applyChatTabOrder`
//! (`src/lib/chat-tab-reorder.ts`) and `peelChatSurface`
//! (`src/lib/chat-surface.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::js;

/// Applies a saved strip order. Unknown/closed codes are skipped. Loop still
/// leads when it is open but missing from the save; other new tabs append.
pub(crate) fn apply_chat_tab_order(tabs: Vec<Value>, order: &[String]) -> Vec<Value> {
    if order.is_empty() || tabs.len() < 2 {
        return tabs;
    }
    let code = |t: &Value| js::get_str(t, "code").to_owned();
    let mut by_code: HashMap<String, &Value> = HashMap::new();
    for t in &tabs {
        by_code.insert(code(t), t);
    }
    let in_order: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(lp) = by_code.get("MA")
        && !in_order.contains("MA")
    {
        out.push((*lp).clone());
        seen.insert("MA".to_owned());
    }
    for c in order {
        let Some(t) = by_code.get(c) else { continue };
        if seen.contains(c) {
            continue;
        }
        out.push((*t).clone());
        seen.insert(c.clone());
    }
    for t in &tabs {
        if !seen.contains(&code(t)) {
            out.push(t.clone());
        }
    }
    out
}

const SURFACE_OPEN: &str = "⟦recur-surface⟧";
const SURFACE_CLOSE: &str = "⟦/recur-surface⟧";

fn trim_start_ws(s: &str) -> &str {
    s.trim_start_matches([' ', '\t', '\r', '\n'])
}

/// Strips a leading looking-at marker. Invalid markers are left in place.
pub(crate) fn peel_chat_surface(text: &str) -> String {
    let s = trim_start_ws(text);
    let Some(rest) = s.strip_prefix(SURFACE_OPEN) else {
        return text.to_owned();
    };
    let Some(close_at) = rest.find(SURFACE_CLOSE) else {
        return text.to_owned();
    };
    let raw = rest[..close_at].trim();
    let ok = serde_json::from_str::<Value>(raw).ok().is_some_and(|v| {
        js::get(&v, "surface")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    });
    if !ok {
        return text.to_owned();
    }
    trim_start_ws(&rest[close_at + SURFACE_CLOSE.len()..]).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codes(tabs: &[Value]) -> Vec<&str> {
        tabs.iter().map(|t| js::get_str(t, "code")).collect()
    }

    #[test]
    fn chat_tab_order() {
        let tabs = vec![
            json!({"code": "MA"}),
            json!({"code": "PA"}),
            json!({"code": "PA2"}),
            json!({"code": "PA3"}),
        ];
        let order = ["PA2".to_owned(), "GONE".to_owned(), "PA".to_owned()];
        assert_eq!(
            codes(&apply_chat_tab_order(tabs.clone(), &order)),
            ["MA", "PA2", "PA", "PA3"]
        );
        let order = ["PA".to_owned(), "MA".to_owned()];
        assert_eq!(
            codes(&apply_chat_tab_order(tabs.clone(), &order)),
            ["PA", "MA", "PA2", "PA3"]
        );
        assert_eq!(
            codes(&apply_chat_tab_order(tabs, &[])),
            ["MA", "PA", "PA2", "PA3"]
        );
    }

    #[test]
    fn chat_surface() {
        let wire =
            "⟦recur-surface⟧{\"surface\":\"github_pr\",\"prId\":\"7\"}⟦/recur-surface⟧\nhello";
        assert_eq!(peel_chat_surface(wire), "hello");
        assert_eq!(
            peel_chat_surface("  \n⟦recur-surface⟧{}⟦/recur-surface⟧x"),
            "  \n⟦recur-surface⟧{}⟦/recur-surface⟧x"
        );
        assert_eq!(peel_chat_surface("plain"), "plain");
        assert_eq!(
            peel_chat_surface("⟦recur-surface⟧{\"surface\":\"workspace\"}"),
            "⟦recur-surface⟧{\"surface\":\"workspace\"}"
        );
    }
}
