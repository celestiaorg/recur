//! The active Agent-panel tab per workspace, kept across reloads under one
//! storage key. The value is a JSON object (`{workspaceId: code}` in
//! insertion order); the view
//! host reads and writes it through its local storage.

use serde::de::{Deserializer, MapAccess, Visitor};

/// The storage key.
pub const ACTIVE_CHAT_TAB_KEY: &str = "recur.activeChatTab.v1";

/// The saved active tab of every workspace, in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveChatTabs(Vec<(String, String)>);

struct Entries(Vec<(String, String)>);

impl<'de> serde::Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entries;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Entries, A::Error> {
                let mut out: Vec<(String, String)> = Vec::new();
                while let Some((k, v)) = m.next_entry::<String, serde_json::Value>()? {
                    // A repeated key keeps its first position, as in JS.
                    let code = match v {
                        serde_json::Value::String(s) if !s.is_empty() => Some(s),
                        _ => None,
                    };
                    match (out.iter().position(|(id, _)| *id == k), code) {
                        (Some(i), Some(c)) => out[i].1 = c,
                        (Some(i), None) => {
                            out.remove(i);
                        }
                        (None, Some(c)) => out.push((k, c)),
                        (None, None) => {}
                    }
                }
                Ok(Entries(out))
            }
        }
        d.deserialize_map(V)
    }
}

impl ActiveChatTabs {
    /// Parses the stored value; anything malformed reads as empty.
    pub fn parse(raw: Option<&str>) -> Self {
        raw.and_then(|r| serde_json::from_str::<Entries>(r).ok())
            .map(|e| Self(e.0))
            .unwrap_or_default()
    }

    /// The value to store, or `None` to remove the key (nothing saved).
    pub fn encode(&self) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        let mut s = String::from("{");
        for (i, (id, code)) in self.0.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&serde_json::to_string(id).unwrap_or_default());
            s.push(':');
            s.push_str(&serde_json::to_string(code).unwrap_or_default());
        }
        s.push('}');
        Some(s)
    }

    pub fn load(&self, workspace_id: &str) -> Option<&str> {
        if workspace_id.is_empty() {
            return None;
        }
        self.0
            .iter()
            .find(|(id, _)| id == workspace_id)
            .map(|(_, c)| c.as_str())
    }

    /// Saves a workspace's tab; true when the stored value changed.
    pub fn save(&mut self, workspace_id: &str, code: &str) -> bool {
        if workspace_id.is_empty() || code.is_empty() {
            return false;
        }
        match self.0.iter_mut().find(|(id, _)| id == workspace_id) {
            Some((_, c)) if c == code => false,
            Some((_, c)) => {
                *c = code.to_owned();
                true
            }
            None => {
                self.0.push((workspace_id.to_owned(), code.to_owned()));
                true
            }
        }
    }

    /// Forgets one workspace; true when it was saved.
    pub fn clear(&mut self, workspace_id: &str) -> bool {
        if workspace_id.is_empty() {
            return false;
        }
        let before = self.0.len();
        self.0.retain(|(id, _)| id != workspace_id);
        self.0.len() != before
    }

    /// Drops workspaces no longer in the live list; true when any went.
    pub fn prune<'a>(&mut self, live: impl IntoIterator<Item = &'a str>) -> bool {
        let live: std::collections::HashSet<&str> = live.into_iter().collect();
        let before = self.0.len();
        self.0.retain(|(id, _)| live.contains(id.as_str()));
        self.0.len() != before
    }

    /// The tab to show for a workspace's strip.
    pub fn restored_chat_code(&self, workspace_id: &str, tab_codes: &[&str]) -> String {
        resolve_active_chat_code(tab_codes, self.load(workspace_id))
    }
}

/// The saved code when it is still in the strip, else the first tab (PA
/// for an empty strip).
pub fn resolve_active_chat_code(tab_codes: &[&str], saved: Option<&str>) -> String {
    if let Some(s) = saved.filter(|s| !s.is_empty() && tab_codes.contains(s)) {
        return s.to_owned();
    }
    tab_codes.first().map_or("PA", |c| *c).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABS: [&str; 3] = ["MA", "PA", "PA2"];

    #[test]
    fn resolve_uses_a_saved_code_still_in_the_strip() {
        assert_eq!(resolve_active_chat_code(&TABS, Some("PA2")), "PA2");
        assert_eq!(resolve_active_chat_code(&TABS, Some("PA")), "PA");
        assert_eq!(resolve_active_chat_code(&TABS, Some("MA")), "MA");
    }

    #[test]
    fn resolve_falls_back_to_the_first_tab() {
        assert_eq!(resolve_active_chat_code(&TABS, None), "MA");
        assert_eq!(resolve_active_chat_code(&TABS, Some("")), "MA");
        assert_eq!(resolve_active_chat_code(&TABS, Some("PA9")), "MA");
        assert_eq!(resolve_active_chat_code(&[], Some("PA")), "PA");
    }

    /// Writes through the stored string each time, as the web UI did.
    fn round(raw: &mut Option<String>, f: impl FnOnce(&mut ActiveChatTabs)) {
        let mut all = ActiveChatTabs::parse(raw.as_deref());
        f(&mut all);
        *raw = all.encode();
    }

    #[test]
    fn round_trips_the_active_code_per_workspace() {
        let mut raw = None;
        round(&mut raw, |a| {
            a.save("ws-a", "PA2");
        });
        round(&mut raw, |a| {
            a.save("ws-b", "PA");
        });
        let all = ActiveChatTabs::parse(raw.as_deref());
        assert_eq!(all.load("ws-a"), Some("PA2"));
        assert_eq!(all.load("ws-b"), Some("PA"));
        assert_eq!(all.load("ws-missing"), None);
        assert_eq!(raw.as_deref(), Some(r#"{"ws-a":"PA2","ws-b":"PA"}"#));
    }

    #[test]
    fn restored_prefers_an_open_saved_tab_and_falls_back() {
        let mut all = ActiveChatTabs::default();
        all.save("ws-a", "PA2");
        assert_eq!(all.restored_chat_code("ws-a", &TABS), "PA2");
        all.save("ws-a", "PA9");
        assert_eq!(all.restored_chat_code("ws-a", &TABS), "MA");
    }

    #[test]
    fn clear_removes_one_workspace() {
        let mut raw = None;
        round(&mut raw, |a| {
            a.save("ws-a", "PA2");
            a.save("ws-b", "PA");
        });
        round(&mut raw, |a| {
            assert!(a.clear("ws-a"));
            assert!(!a.clear("ws-missing"));
        });
        let all = ActiveChatTabs::parse(raw.as_deref());
        assert_eq!(all.load("ws-a"), None);
        assert_eq!(all.load("ws-b"), Some("PA"));
        round(&mut raw, |a| {
            a.clear("ws-b");
        });
        assert_eq!(raw, None);
    }

    #[test]
    fn prune_drops_deleted_workspaces() {
        let mut all = ActiveChatTabs::default();
        all.save("ws-a", "PA2");
        all.save("ws-b", "PA");
        all.save("ws-gone", "MA");
        assert!(all.prune(["ws-a", "ws-b"]));
        assert_eq!(all.load("ws-a"), Some("PA2"));
        assert_eq!(all.load("ws-b"), Some("PA"));
        assert_eq!(all.load("ws-gone"), None);
        assert!(!all.prune(["ws-a", "ws-b"]));
    }

    #[test]
    fn malformed_values_read_empty_and_skip_non_strings() {
        assert_eq!(
            ActiveChatTabs::parse(Some("nope")),
            ActiveChatTabs::default()
        );
        assert_eq!(
            ActiveChatTabs::parse(Some("[1]")),
            ActiveChatTabs::default()
        );
        let all = ActiveChatTabs::parse(Some(r#"{"a":"PA","b":3,"c":""}"#));
        assert_eq!(all.encode().as_deref(), Some(r#"{"a":"PA"}"#));
    }
}
