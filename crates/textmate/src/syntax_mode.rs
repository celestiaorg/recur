//! User-chosen TextMate language per file (`syntax-mode.ts`).
//!
//! Path overrides and `*.ext` / basename associations, persisted on the
//! workspace editor session by the app. Maps keep insertion order (JS object
//! semantics), which decides what the size caps evict.

use crate::node::Node;

/// Empty string = explicit Plain Text. A missing key = auto-detect.
pub const SYNTAX_PLAIN: &str = "";
/// Max persisted per-path overrides (oldest dropped).
pub const MAX_SYNTAX_BY_PATH: usize = 256;
/// Max persisted `*.ext` / basename associations.
pub const MAX_SYNTAX_BY_FILE_TYPE: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTypeAssociation {
    pub key: String,
    pub label: String,
}

/// An insertion-ordered string map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderedMap(pub Vec<(String, String)>);

impl OrderedMap {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }

    pub fn contains_key(&self, k: &str) -> bool {
        self.0.iter().any(|(key, _)| key == k)
    }

    fn remove(&mut self, k: &str) {
        self.0.retain(|(key, _)| key != k);
    }

    /// JS assignment: an existing key keeps its position.
    fn assign(&mut self, k: &str, v: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v.to_string(),
            None => self.0.push((k.to_string(), v.to_string())),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn same_entries(&self, other: &OrderedMap) -> bool {
        self.len() == other.len() && self.0.iter().all(|(k, v)| other.get(k) == Some(v.as_str()))
    }

    fn capped(&self, max: usize) -> OrderedMap {
        let skip = self.0.len().saturating_sub(max);
        OrderedMap(self.0[skip..].to_vec())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyntaxModeMaps {
    pub syntax_by_path: OrderedMap,
    pub syntax_by_file_type: OrderedMap,
}

/// Parse a persisted map object (non-string values and empty keys dropped).
pub fn parse_syntax_map(raw: Option<&Node>) -> OrderedMap {
    let mut out = OrderedMap::default();
    if let Some(node @ Node::Map(_)) = raw {
        for (k, v) in node.js_entries() {
            if let (false, Some(s)) = (k.is_empty(), v.as_str()) {
                out.assign(&k, s);
            }
        }
    }
    out
}

pub fn normalize_syntax_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.strip_prefix("./").map(str::to_string).unwrap_or(p)
}

/// Association key: dotfiles and extensionless names use the basename,
/// everything else `*.ext`.
pub fn file_type_association_for_path(path: &str) -> Option<FileTypeAssociation> {
    if path.starts_with("scratch://") {
        return None;
    }
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
    if base.is_empty() {
        return None;
    }
    let lower = base.to_lowercase();
    if lower.starts_with('.') {
        return Some(FileTypeAssociation {
            key: lower,
            label: base.to_string(),
        });
    }
    if let Some(dot) = lower.rfind('.')
        && dot > 0
        && dot + 1 < lower.len()
    {
        let key = format!("*.{}", &lower[dot + 1..]);
        return Some(FileTypeAssociation {
            label: key.clone(),
            key,
        });
    }
    Some(FileTypeAssociation {
        key: lower,
        label: base.to_string(),
    })
}

pub fn prune_syntax_maps(maps: &SyntaxModeMaps) -> SyntaxModeMaps {
    let by_path = OrderedMap(
        maps.syntax_by_path
            .0
            .iter()
            .filter(|(k, _)| !k.starts_with("scratch://"))
            .cloned()
            .collect(),
    );
    SyntaxModeMaps {
        syntax_by_path: by_path.capped(MAX_SYNTAX_BY_PATH),
        syntax_by_file_type: maps.syntax_by_file_type.capped(MAX_SYNTAX_BY_FILE_TYPE),
    }
}

fn put_capped(map: &mut OrderedMap, key: &str, value: &str, max: usize) {
    map.remove(key);
    map.0.push((key.to_string(), value.to_string()));
    if map.len() > max {
        map.0.remove(0);
    }
}

/// The in-memory resolver. `epoch` bumps on every change; `persist` runs
/// after user picks (never on hydrate).
#[derive(Default)]
pub struct SyntaxMode {
    by_path: OrderedMap,
    by_file_type: OrderedMap,
    epoch: u64,
    persist: Option<Box<dyn FnMut()>>,
}

impl SyntaxMode {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn set_persist(&mut self, hook: Option<Box<dyn FnMut()>>) {
        self.persist = hook;
    }

    fn bump(&mut self) {
        self.epoch += 1;
        if let Some(p) = self.persist.as_mut() {
            p();
        }
    }

    /// `None` = auto-detect; `Some("")` = Plain Text; otherwise a scope.
    pub fn override_for_path(&self, path: &str) -> Option<String> {
        let p = normalize_syntax_path(path);
        if p.is_empty() {
            return None;
        }
        if let Some(v) = self.by_path.get(&p) {
            return Some(v.to_string());
        }
        let assoc = file_type_association_for_path(&p)?;
        self.by_file_type.get(&assoc.key).map(str::to_string)
    }

    pub fn path_override(&self, path: &str) -> Option<String> {
        let p = normalize_syntax_path(path);
        if p.is_empty() {
            return None;
        }
        self.by_path.get(&p).map(str::to_string)
    }

    pub fn file_type_override(&self, key: &str) -> Option<String> {
        self.by_file_type.get(key).map(str::to_string)
    }

    pub fn snapshot(&self) -> SyntaxModeMaps {
        prune_syntax_maps(&SyntaxModeMaps {
            syntax_by_path: self.by_path.clone(),
            syntax_by_file_type: self.by_file_type.clone(),
        })
    }

    pub fn maps_non_empty(&self) -> bool {
        let s = self.snapshot();
        !s.syntax_by_path.is_empty() || !s.syntax_by_file_type.is_empty()
    }

    /// Replace maps from a session load. `merge_live` keeps picks made since.
    pub fn hydrate(&mut self, by_path: OrderedMap, by_file_type: OrderedMap, merge_live: bool) {
        let (mut next_path, mut next_type) = (by_path, by_file_type);
        if merge_live {
            for (k, v) in &self.by_path.0 {
                next_path.assign(k, v);
            }
            for (k, v) in &self.by_file_type.0 {
                next_type.assign(k, v);
            }
        }
        if self.by_path.same_entries(&next_path) && self.by_file_type.same_entries(&next_type) {
            return;
        }
        self.by_path = next_path;
        self.by_file_type = next_type;
        self.epoch += 1;
    }

    /// Set or clear (`None` = Auto Detect) the per-file override.
    pub fn set_for_path(&mut self, path: &str, scope: Option<&str>) {
        let p = normalize_syntax_path(path);
        if p.is_empty() {
            return;
        }
        match scope {
            None => {
                if !self.by_path.contains_key(&p) {
                    return;
                }
                self.by_path.remove(&p);
            }
            Some(s) if self.by_path.get(&p) == Some(s) => return,
            Some(s) => put_capped(&mut self.by_path, &p, s, MAX_SYNTAX_BY_PATH),
        }
        self.bump();
    }

    /// Set or clear a file-type association.
    pub fn set_for_file_type(&mut self, key: &str, scope: Option<&str>) {
        let k = key.trim();
        if k.is_empty() {
            return;
        }
        match scope {
            None => {
                if !self.by_file_type.contains_key(k) {
                    return;
                }
                self.by_file_type.remove(k);
            }
            Some(s) if self.by_file_type.get(k) == Some(s) => return,
            Some(s) => put_capped(&mut self.by_file_type, k, s, MAX_SYNTAX_BY_FILE_TYPE),
        }
        self.bump();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    fn om(pairs: &[(&str, &str)]) -> OrderedMap {
        OrderedMap(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn assoc(key: &str, label: &str) -> Option<FileTypeAssociation> {
        Some(FileTypeAssociation {
            key: key.into(),
            label: label.into(),
        })
    }

    #[test]
    fn file_type_associations() {
        assert_eq!(
            file_type_association_for_path("src/main.go"),
            assoc("*.go", "*.go")
        );
        assert_eq!(
            file_type_association_for_path("foo.JSON"),
            assoc("*.json", "*.json")
        );
        assert_eq!(
            file_type_association_for_path(".env.local"),
            assoc(".env.local", ".env.local")
        );
        assert_eq!(
            file_type_association_for_path("Makefile"),
            assoc("makefile", "Makefile")
        );
        assert_eq!(file_type_association_for_path("scratch://1-Scratch"), None);
        assert_eq!(
            file_type_association_for_path("scratch://1-notes.txt"),
            None
        );
    }

    #[test]
    fn path_override_wins_over_file_type() {
        let mut m = SyntaxMode::default();
        m.set_for_file_type("*.go", Some("source.python"));
        assert_eq!(
            m.override_for_path("src/main.go").as_deref(),
            Some("source.python")
        );
        m.set_for_path("src/main.go", Some("source.yaml"));
        assert_eq!(
            m.override_for_path("src/main.go").as_deref(),
            Some("source.yaml")
        );
        assert_eq!(
            m.override_for_path("lib/other.go").as_deref(),
            Some("source.python")
        );
    }

    #[test]
    fn empty_string_is_plain_text_and_auto_detect_deletes() {
        let mut m = SyntaxMode::default();
        m.set_for_path("src/main.go", Some(SYNTAX_PLAIN));
        assert_eq!(m.override_for_path("src/main.go").as_deref(), Some(""));
        assert_eq!(m.path_override("src/main.go").as_deref(), Some(""));
        m.set_for_path("src/main.go", Some("source.yaml"));
        m.set_for_path("src/main.go", None);
        assert_eq!(m.override_for_path("src/main.go"), None);
        assert_eq!(m.path_override("src/main.go"), None);
    }

    #[test]
    fn hydrate_replaces_maps_without_persisting() {
        let n = Rc::new(Cell::new(0));
        let mut m = SyntaxMode::default();
        let c = n.clone();
        m.set_persist(Some(Box::new(move || c.set(c.get() + 1))));
        m.set_for_path("notes.txt", Some("text.html.markdown"));
        m.set_for_file_type("*.conf", Some("source.yaml"));
        assert_eq!(n.get(), 2);
        m.hydrate(
            om(&[("notes.txt", "text.html.markdown"), ("src/main.go", "")]),
            om(&[("*.conf", "source.yaml")]),
            false,
        );
        assert_eq!(n.get(), 2);
        assert_eq!(
            m.override_for_path("notes.txt").as_deref(),
            Some("text.html.markdown")
        );
        assert_eq!(m.override_for_path("src/main.go").as_deref(), Some(""));
        assert_eq!(
            m.file_type_override("*.conf").as_deref(),
            Some("source.yaml")
        );
        assert!(m.maps_non_empty());
    }

    #[test]
    fn snapshot_is_a_copy_and_empty_hydrate_clears() {
        let mut m = SyntaxMode::default();
        m.set_for_path("a.go", Some("source.yaml"));
        let mut snap = m.snapshot();
        snap.syntax_by_path.assign("a.go", "nope");
        assert_eq!(m.override_for_path("a.go").as_deref(), Some("source.yaml"));
        m.hydrate(OrderedMap::default(), OrderedMap::default(), false);
        assert_eq!(m.override_for_path("a.go"), None);
        assert!(!m.maps_non_empty());
    }

    #[test]
    fn merge_live_keeps_picks_after_an_empty_hydrate() {
        let mut m = SyntaxMode::default();
        m.hydrate(OrderedMap::default(), OrderedMap::default(), false);
        m.set_for_path("src/main.go", Some("source.yaml"));
        m.hydrate(
            om(&[("README.md", "source.yaml")]),
            om(&[("*.conf", "source.yaml")]),
            true,
        );
        assert_eq!(
            m.override_for_path("src/main.go").as_deref(),
            Some("source.yaml")
        );
        assert_eq!(
            m.override_for_path("README.md").as_deref(),
            Some("source.yaml")
        );
        assert_eq!(
            m.file_type_override("*.conf").as_deref(),
            Some("source.yaml")
        );
    }

    #[test]
    fn snapshot_drops_scratch_paths() {
        let mut m = SyntaxMode::default();
        m.set_for_path("scratch://1-Scratch", Some("source.yaml"));
        m.set_for_path("src/main.go", Some("source.yaml"));
        assert_eq!(
            m.snapshot().syntax_by_path,
            om(&[("src/main.go", "source.yaml")])
        );
    }

    #[test]
    fn caps_per_path_overrides_dropping_the_oldest() {
        let mut m = SyntaxMode::default();
        for i in 0..MAX_SYNTAX_BY_PATH + 3 {
            m.set_for_path(&format!("f{i}.txt"), Some("source.yaml"));
        }
        assert_eq!(m.path_override("f0.txt"), None);
        assert_eq!(
            m.path_override(&format!("f{}.txt", MAX_SYNTAX_BY_PATH + 2))
                .as_deref(),
            Some("source.yaml")
        );
        assert_eq!(m.snapshot().syntax_by_path.len(), MAX_SYNTAX_BY_PATH);
    }

    #[test]
    fn parses_persisted_maps_in_document_order() {
        let n = Node::parse_json(r#"{"b.go":"x","a.go":"y","bad":1,"":"z"}"#).unwrap();
        assert_eq!(
            parse_syntax_map(Some(&n)),
            om(&[("b.go", "x"), ("a.go", "y")])
        );
    }
}
