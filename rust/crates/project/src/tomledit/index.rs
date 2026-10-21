//! The span index over a document: which bytes express each key-value,
//! table and array-of-tables element. Rebuilt after every mutation.

use std::collections::HashMap;

use super::syntax::{Expr, KeyPart, Node, Span, expressions};

/// A key-value in the document.
#[derive(Debug, Clone)]
pub(crate) struct Leaf {
    /// The whole key-value expression: from the start of the full-line
    /// comments attached above it to past the end of its last line
    /// (including any comment trailing the value).
    pub line: Span,
    /// Exactly the bytes of the value.
    pub value: Span,
    /// The value's syntax, for edits that descend into arrays and inline
    /// tables.
    pub node: Node,
}

#[derive(Debug, Clone)]
pub(crate) enum Item {
    Leaf(Leaf),
    Table(usize),
    /// An array of tables: one table per `[[element]]`.
    Array(Vec<usize>),
}

/// A table of the document: the root, a `[table]` section, an element of an
/// `[[array table]]`, or a table implied by a header or a dotted key.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    pub path: Vec<String>,
    pub parent: Option<usize>,
    pub root: bool,
    /// Explicit tables have their own section: a `[header]` (or `[[header]]`)
    /// followed by key-values.
    pub explicit: bool,
    /// Set on array-table elements and everything below them: their absolute
    /// path is only meaningful relative to the last element of each array,
    /// so new sub-tables cannot be created with `[headers]`.
    pub via_array: bool,
    /// For explicit tables: header start (incl. attached comments) to end of
    /// section.
    pub section: Span,
    /// Where to insert a new key-value line (explicit tables and the root).
    pub insert_at: Option<usize>,
    /// Defined by a dotted key inside the section of the nearest section
    /// ancestor; extended with dotted keys.
    pub dotted: bool,
    pub items: HashMap<String, Item>,
}

#[derive(Debug, Clone)]
pub(crate) struct Index {
    /// `tables[0]` is the root.
    pub tables: Vec<Table>,
}

impl Index {
    fn new_table(&mut self, parent: Option<usize>, name: &str) -> usize {
        let (path, via_array) = match parent {
            Some(p) => {
                let mut path = self.tables[p].path.clone();
                path.push(name.to_owned());
                (path, self.tables[p].via_array)
            }
            None => (Vec::new(), false),
        };
        self.tables.push(Table {
            path,
            parent,
            root: false,
            explicit: false,
            via_array,
            section: 0..0,
            insert_at: None,
            dotted: false,
            items: HashMap::new(),
        });
        self.tables.len() - 1
    }

    /// The closest ancestor-or-self of t that has its own section to insert
    /// key-values into: an explicit table or the root.
    pub fn nearest_section(&self, mut t: usize) -> usize {
        while !self.tables[t].explicit && !self.tables[t].root {
            match self.tables[t].parent {
                Some(p) => t = p,
                None => break,
            }
        }
        t
    }

    /// The offset right after the last byte that belongs to t or any of its
    /// descendants: where a new section for a child of t is inserted.
    pub fn subtree_end(&self, t: usize) -> usize {
        let mut end = self.tables[t].section.end;
        for it in self.tables[t].items.values() {
            let e = match it {
                Item::Leaf(lf) => lf.line.end,
                Item::Table(c) => self.subtree_end(*c),
                Item::Array(arr) => self.array_subtree_end(arr),
            };
            end = end.max(e);
        }
        end
    }

    pub fn array_subtree_end(&self, arr: &[usize]) -> usize {
        arr.iter().map(|e| self.subtree_end(*e)).max().unwrap_or(0)
    }

    /// Returns the table for a `[header]`, creating implicit intermediate
    /// tables. Traversing an array of tables descends into its last element.
    fn resolve_header(&mut self, parts: &[String]) -> Result<usize, String> {
        let mut t = 0;
        for (i, name) in parts.iter().enumerate() {
            t = match self.tables[t].items.get(name) {
                None => {
                    let nt = self.new_table(Some(t), name);
                    self.tables[t].items.insert(name.clone(), Item::Table(nt));
                    nt
                }
                Some(Item::Table(c)) => *c,
                Some(Item::Array(arr)) if i < parts.len() - 1 => arr[arr.len() - 1],
                Some(_) => {
                    return Err(format!(
                        "toml/edit: internal error: header {parts:?} redefines a value"
                    ));
                }
            };
        }
        Ok(t)
    }

    /// Appends a new element table to the array of tables designated by an
    /// `[[header]]`, creating the array if needed.
    fn append_array_element(&mut self, parts: &[String]) -> Result<usize, String> {
        let mut t = 0;
        for name in &parts[..parts.len() - 1] {
            t = match self.tables[t].items.get(name) {
                None => {
                    let nt = self.new_table(Some(t), name);
                    self.tables[t].items.insert(name.clone(), Item::Table(nt));
                    nt
                }
                Some(Item::Table(c)) => *c,
                Some(Item::Array(arr)) => arr[arr.len() - 1],
                Some(Item::Leaf(_)) => {
                    return Err(format!(
                        "toml/edit: internal error: array table header {parts:?} crosses a value"
                    ));
                }
            };
        }
        let name = &parts[parts.len() - 1];
        let elem = self.new_table(Some(t), name);
        self.tables[elem].explicit = true;
        self.tables[elem].via_array = true;
        match self.tables[t].items.get_mut(name) {
            None => {
                self.tables[t]
                    .items
                    .insert(name.clone(), Item::Array(vec![elem]));
            }
            Some(Item::Array(arr)) => arr.push(elem),
            Some(_) => {
                return Err(format!(
                    "toml/edit: internal error: array table header {parts:?} redefines a table or value"
                ));
            }
        }
        Ok(elem)
    }

    /// Records a key-value parsed in the section of table sec, creating the
    /// tables implied by dotted key parts.
    fn add_key_value(&mut self, sec: usize, parts: &[String], lf: Leaf) -> Result<(), String> {
        let mut t = sec;
        for name in &parts[..parts.len() - 1] {
            t = match self.tables[t].items.get(name) {
                None => {
                    let nt = self.new_table(Some(t), name);
                    self.tables[nt].dotted = true;
                    self.tables[t].items.insert(name.clone(), Item::Table(nt));
                    nt
                }
                Some(Item::Table(c)) => *c,
                Some(_) => {
                    return Err(format!(
                        "toml/edit: internal error: dotted key {parts:?} crosses a value"
                    ));
                }
            };
        }
        let name = &parts[parts.len() - 1];
        if self.tables[t].items.contains_key(name) {
            return Err(format!(
                "toml/edit: internal error: duplicate key {parts:?}"
            ));
        }
        self.tables[t].items.insert(name.clone(), Item::Leaf(lf));
        Ok(())
    }
}

fn names(keys: &[KeyPart]) -> Vec<String> {
    keys.iter().map(|k| k.name.clone()).collect()
}

/// The offset of the first byte of the line off is on.
pub(crate) fn line_start(data: &str, off: usize) -> usize {
    data.as_bytes()[..off]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1)
}

/// The offset right past the newline ending the line off is on, or the end
/// of the document.
pub(crate) fn line_end(data: &str, off: usize) -> usize {
    data.as_bytes()[off..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(data.len(), |i| off + i + 1)
}

/// Builds the span index for data, which must be a valid TOML document.
pub(crate) fn reindex(data: &str) -> Result<Index, String> {
    let mut idx = Index { tables: Vec::new() };
    let root = idx.new_table(None, "");
    idx.tables[root].root = true;
    idx.tables[root].section = 0..data.len();

    let mut current = root;
    let mut first_header_attach: Option<usize> = None;
    // The current run of contiguous full-line comments. A run that ends on
    // the line right above an expression is attached to it: it moves, or is
    // removed, with it.
    let mut run: Option<(usize, usize)> = None;

    for e in expressions(data) {
        let span = match &e {
            Expr::Comment(span) => {
                let ls = line_start(data, span.start);
                let le = line_end(data, span.end);
                run = match run {
                    Some((start, end)) if end == ls => Some((start, le)),
                    _ => Some((ls, le)),
                };
                continue;
            }
            Expr::KeyValue { span, .. } | Expr::Table { span, .. } | Expr::ArrayTable { span, .. } => {
                span.clone()
            }
        };
        let ls = line_start(data, span.start);
        let le = line_end(data, span.end);
        let attach = match run {
            Some((start, end)) if end == ls => start,
            _ => ls,
        };
        run = None;

        match e {
            Expr::KeyValue { keys, value, .. } => {
                let lf = Leaf {
                    line: attach..le,
                    value: value.span(),
                    node: value,
                };
                idx.add_key_value(current, &names(&keys), lf)?;
                idx.tables[current].insert_at = Some(le);
            }
            Expr::Table { keys, .. } => {
                idx.tables[current].section.end = attach;
                let t = idx.resolve_header(&names(&keys))?;
                let tb = &mut idx.tables[t];
                tb.explicit = true;
                tb.section = attach..data.len();
                tb.insert_at = Some(le);
                first_header_attach.get_or_insert(attach);
                current = t;
            }
            Expr::ArrayTable { keys, .. } => {
                idx.tables[current].section.end = attach;
                let elem = idx.append_array_element(&names(&keys))?;
                let tb = &mut idx.tables[elem];
                tb.section = attach..data.len();
                tb.insert_at = Some(le);
                first_header_attach.get_or_insert(attach);
                current = elem;
            }
            Expr::Comment(_) => {}
        }
    }
    if current != root {
        idx.tables[current].section.end = data.len();
    }
    if idx.tables[root].insert_at.is_none() {
        // No top-level key-value: new ones go before the first section
        // (comments attached to its header included), or at the end of a
        // document without sections.
        idx.tables[root].insert_at = Some(first_header_attach.unwrap_or(data.len()));
    }
    Ok(idx)
}

/// Interprets a key path element as an array index: a non-empty string of
/// at most 18 decimal digits.
pub(crate) fn parse_index(s: &str) -> Option<usize> {
    if s.is_empty() || s.len() > 18 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
