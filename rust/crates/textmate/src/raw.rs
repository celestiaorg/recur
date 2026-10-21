//! Raw grammar model (`rawGrammar.ts`) and `parseRawGrammar`.
//!
//! Rules are shared by `Rc` the way JS shares object references: a rule
//! reachable from several repositories is one object and is compiled once
//! (its id is cached on the rule). Each `Grammar` works on its own deep copy
//! (`initGrammar` clones), so ids never leak between grammars.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::node::Node;
use crate::plist::{PlistError, parse_plist};

pub type RawRepository = Rc<HashMap<String, Rc<RawRule>>>;

#[derive(Default, Debug)]
pub struct RawRule {
    /// Compiled rule id within the owning grammar (0 = not compiled yet).
    pub(crate) id: Cell<u32>,
    pub include: Option<String>,
    pub name: Option<String>,
    pub content_name: Option<String>,
    pub match_: Option<String>,
    pub captures: Option<RawCaptures>,
    pub begin: Option<String>,
    pub begin_captures: Option<RawCaptures>,
    pub end: Option<String>,
    pub end_captures: Option<RawCaptures>,
    pub while_: Option<String>,
    pub while_captures: Option<RawCaptures>,
    pub patterns: Option<Vec<Rc<RawRule>>>,
    pub repository: Option<RawRepository>,
    pub apply_end_pattern_last: bool,
}

/// Capture rules keyed by the original property name, in JS iteration order.
#[derive(Default, Debug, Clone)]
pub struct RawCaptures(pub Vec<(String, Rc<RawRule>)>);

#[derive(Debug)]
pub struct RawGrammar {
    pub repository: RawRepository,
    pub scope_name: String,
    pub patterns: Option<Vec<Rc<RawRule>>>,
    /// Injection selector -> rule, in JS iteration order.
    pub injections: Option<Vec<(String, Rc<RawRule>)>>,
    pub injection_selector: Option<String>,
    pub file_types: Vec<String>,
    pub name: Option<String>,
    pub first_line_match: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GrammarParseError {
    #[error("invalid grammar JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid grammar plist: {0}")]
    Plist(#[from] PlistError),
    #[error("grammar is not an object")]
    NotAnObject,
}

fn opt_string(n: &Node, key: &str) -> Option<String> {
    n.str_field(key).map(str::to_string)
}

fn rule_list(n: Option<&Node>) -> Option<Vec<Rc<RawRule>>> {
    match n {
        Some(Node::Array(items)) => Some(
            items
                .iter()
                .map(|i| Rc::new(RawRule::from_node(i)))
                .collect(),
        ),
        _ => None,
    }
}

fn captures(n: Option<&Node>) -> Option<RawCaptures> {
    match n {
        Some(node @ (Node::Map(_) | Node::Array(_))) => Some(RawCaptures(
            node.js_entries()
                .into_iter()
                .map(|(k, v)| (k, Rc::new(RawRule::from_node(v))))
                .collect(),
        )),
        _ => None,
    }
}

fn repository(n: Option<&Node>) -> Option<RawRepository> {
    match n {
        Some(Node::Map(entries)) => Some(Rc::new(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), Rc::new(RawRule::from_node(v))))
                .collect(),
        )),
        _ => None,
    }
}

impl RawRule {
    pub fn from_node(n: &Node) -> RawRule {
        RawRule {
            id: Cell::new(0),
            include: opt_string(n, "include"),
            name: opt_string(n, "name"),
            content_name: opt_string(n, "contentName"),
            match_: opt_string(n, "match"),
            captures: captures(n.get("captures")),
            begin: opt_string(n, "begin"),
            begin_captures: captures(n.get("beginCaptures")),
            end: opt_string(n, "end"),
            end_captures: captures(n.get("endCaptures")),
            while_: opt_string(n, "while"),
            while_captures: captures(n.get("whileCaptures")),
            patterns: rule_list(n.get("patterns")),
            repository: repository(n.get("repository")),
            apply_end_pattern_last: n.get("applyEndPatternLast").is_some_and(Node::truthy),
        }
    }

    fn deep_clone(&self) -> RawRule {
        let caps = |c: &Option<RawCaptures>| {
            c.as_ref().map(|c| {
                RawCaptures(
                    c.0.iter()
                        .map(|(k, r)| (k.clone(), Rc::new(r.deep_clone())))
                        .collect(),
                )
            })
        };
        RawRule {
            id: Cell::new(self.id.get()),
            include: self.include.clone(),
            name: self.name.clone(),
            content_name: self.content_name.clone(),
            match_: self.match_.clone(),
            captures: caps(&self.captures),
            begin: self.begin.clone(),
            begin_captures: caps(&self.begin_captures),
            end: self.end.clone(),
            end_captures: caps(&self.end_captures),
            while_: self.while_.clone(),
            while_captures: caps(&self.while_captures),
            patterns: self
                .patterns
                .as_ref()
                .map(|p| p.iter().map(|r| Rc::new(r.deep_clone())).collect()),
            repository: self.repository.as_ref().map(|r| Rc::new(clone_repo(r))),
            apply_end_pattern_last: self.apply_end_pattern_last,
        }
    }
}

fn clone_repo(r: &HashMap<String, Rc<RawRule>>) -> HashMap<String, Rc<RawRule>> {
    r.iter()
        .map(|(k, v)| (k.clone(), Rc::new(v.deep_clone())))
        .collect()
}

impl RawGrammar {
    pub fn from_node(n: &Node) -> Result<RawGrammar, GrammarParseError> {
        if !matches!(n, Node::Map(_)) {
            return Err(GrammarParseError::NotAnObject);
        }
        let injections = match n.get("injections") {
            Some(node @ Node::Map(_)) => Some(
                node.js_entries()
                    .into_iter()
                    .map(|(k, v)| (k, Rc::new(RawRule::from_node(v))))
                    .collect(),
            ),
            _ => None,
        };
        let file_types = match n.get("fileTypes") {
            Some(Node::Array(items)) => items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        Ok(RawGrammar {
            repository: repository(n.get("repository")).unwrap_or_default(),
            scope_name: opt_string(n, "scopeName").unwrap_or_default(),
            patterns: rule_list(n.get("patterns")),
            injections,
            injection_selector: opt_string(n, "injectionSelector"),
            file_types,
            name: opt_string(n, "name"),
            first_line_match: opt_string(n, "firstLineMatch"),
        })
    }

    /// `initGrammar`: a deep copy with `$self` and `$base` in the repository.
    pub(crate) fn init(&self, base: Option<Rc<RawRule>>) -> RawGrammar {
        let patterns = self.patterns.as_ref().map(|p| {
            p.iter()
                .map(|r| Rc::new(r.deep_clone()))
                .collect::<Vec<_>>()
        });
        let injections = self.injections.as_ref().map(|inj| {
            inj.iter()
                .map(|(k, r)| (k.clone(), Rc::new(r.deep_clone())))
                .collect()
        });
        let mut repo = clone_repo(&self.repository);
        let self_rule = Rc::new(RawRule {
            patterns: patterns.clone(),
            name: Some(self.scope_name.clone()),
            ..RawRule::default()
        });
        repo.insert("$self".into(), self_rule.clone());
        repo.insert("$base".into(), base.unwrap_or(self_rule));
        RawGrammar {
            repository: Rc::new(repo),
            scope_name: self.scope_name.clone(),
            patterns,
            injections,
            injection_selector: self.injection_selector.clone(),
            file_types: self.file_types.clone(),
            name: self.name.clone(),
            first_line_match: self.first_line_match.clone(),
        }
    }
}

/// `parseRawGrammar`: JSON when the path ends in `.json`, plist otherwise.
pub fn parse_raw_grammar(
    content: &str,
    file_path: Option<&str>,
) -> Result<RawGrammar, GrammarParseError> {
    let node = match file_path {
        Some(p) if p.ends_with(".json") => Node::parse_json(content)?,
        _ => parse_plist(content)?,
    };
    RawGrammar::from_node(&node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_grammar_with_ordered_injections() {
        let g = parse_raw_grammar(
            r#"{"scopeName":"source.x","patterns":[{"match":"a","name":"n"}],
               "injections":{"R:b":{"match":"b"},"L:a":{"match":"a"}},
               "repository":{"r":{"begin":"(","end":")","applyEndPatternLast":1}}}"#,
            Some("x.tmLanguage.json"),
        )
        .unwrap();
        assert_eq!(g.scope_name, "source.x");
        let inj: Vec<&str> = g
            .injections
            .as_ref()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(inj, ["R:b", "L:a"]);
        assert!(g.repository["r"].apply_end_pattern_last);
    }

    #[test]
    fn init_adds_self_and_base_without_sharing_ids() {
        let g = parse_raw_grammar(
            r#"{"scopeName":"source.x","patterns":[{"match":"a"}]}"#,
            Some("x.json"),
        )
        .unwrap();
        let a = g.init(None);
        let b = g.init(None);
        a.repository["$self"].patterns.as_ref().unwrap()[0]
            .id
            .set(7);
        assert_eq!(
            b.repository["$self"].patterns.as_ref().unwrap()[0].id.get(),
            0
        );
        assert!(Rc::ptr_eq(&a.repository["$self"], &a.repository["$base"]));
        assert!(Rc::ptr_eq(
            &a.repository["$self"].patterns.as_ref().unwrap()[0],
            &a.patterns.as_ref().unwrap()[0]
        ));
    }
}
