//! Ordered document tree for grammar files (JSON and plist).
//!
//! Key order matters: vscode-textmate walks `injections` with `for ... in`,
//! so selector order decides ties between injections. JS objects iterate
//! integer-like keys ascending, then string keys in insertion order; `Map`
//! keeps document order and [`Node::js_keys`] applies the JS rule.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Node::as_str)
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Node::Null => false,
            Node::Bool(b) => *b,
            Node::Number(n) => *n != 0.0 && !n.is_nan(),
            Node::String(s) => !s.is_empty(),
            Node::Array(_) | Node::Map(_) => true,
        }
    }

    /// Entries in JS `for ... in` order: array indices, or object keys with
    /// integer-like keys first (ascending) and the rest in insertion order.
    pub fn js_entries(&self) -> Vec<(String, &Node)> {
        match self {
            Node::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
            Node::Map(entries) => {
                let mut ints: Vec<(u32, &str, &Node)> = Vec::new();
                let mut rest: Vec<(String, &Node)> = Vec::new();
                for (k, v) in entries {
                    match array_index(k) {
                        Some(i) => ints.push((i, k, v)),
                        None => rest.push((k.clone(), v)),
                    }
                }
                ints.sort_by_key(|(i, _, _)| *i);
                ints.into_iter()
                    .map(|(_, k, v)| (k.to_string(), v))
                    .chain(rest)
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn parse_json(text: &str) -> Result<Node, serde_json::Error> {
        let mut de = serde_json::Deserializer::from_str(text);
        let node = Node::deserialize(&mut de)?;
        de.end()?;
        Ok(node)
    }
}

/// A canonical array index as JS defines it (`"0"`, `"12"`, not `"01"`).
fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) {
        return None;
    }
    if !k.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    k.parse::<u32>().ok().filter(|n| *n != u32::MAX)
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Node, E> {
        Ok(Node::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Number(v as f64))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Node, E> {
        Ok(Node::Number(v as f64))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Node, E> {
        Ok(Node::Number(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::String(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<Node, E> {
        Ok(Node::String(v))
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element()? {
            out.push(v);
        }
        Ok(Node::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut out: Vec<(String, Node)> = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, Node>()? {
            // JSON.parse: a repeated key keeps its first position, last value.
            match out.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => out.push((k, v)),
            }
        }
        Ok(Node::Map(out))
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Node, D::Error> {
        d.deserialize_any(NodeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_insertion_order_and_js_key_order() {
        let n = Node::parse_json(r#"{"b":1,"2":2,"a":3,"10":4,"1":5}"#).unwrap();
        let keys: Vec<String> = n.js_entries().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["1", "2", "10", "b", "a"]);
    }

    #[test]
    fn duplicate_keys_keep_first_position_last_value() {
        let n = Node::parse_json(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(
            n,
            Node::Map(vec![
                ("a".into(), Node::Number(3.0)),
                ("b".into(), Node::Number(2.0)),
            ])
        );
    }
}
