//! Comment- and layout-preserving editing of TOML documents, a port of
//! go-toml v2's `unstable/edit` (the subset project.toml writes use).
//!
//! [`Document::to_string`] returns the source byte-for-byte identical to the
//! input except for the parts modified through [`Document::set`] and
//! [`Document::delete`]: an edit rewrites only the bytes that express it.
//!
//! Values are addressed by key path, one element per key part. When a path
//! element steps into an array it is a 0-based decimal index; for `set`, an
//! index equal to the array's length appends a new element (a new
//! `[[table]]` section for arrays of tables).
//!
//! Comments travel with the expression they annotate: the contiguous
//! full-line comments directly above a key-value or header, and the comment
//! trailing it, move and are deleted with it.
//!
//! Every mutation is validated: if an edit would produce an invalid
//! document, the document is left unchanged and an error is returned.
//!
//! go-toml's own editor is built on its streaming parser; this one gets the
//! same expression spans from `toml_parser` (toml_edit's parser). toml_edit's
//! `DocumentMut` is not used for writing because it attaches blank lines and
//! detached comments to the following key, so its inserts and deletes move
//! different bytes than go-toml's.

mod encode;
mod index;
mod inline;
mod syntax;

pub(crate) use encode::{array_tables, key_value, multiline_field, string as encode_string};

use index::{Index, Item, parse_index, reindex};
use syntax::Span;

/// A value to write with [`Document::set`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string, rendered like go-toml's `Marshal` in inline form.
    Str(String),
    /// Exact TOML for the value, used verbatim (go-toml's
    /// `unstable.RawMessage`).
    Raw(String),
    /// A table, rendered inline with keys sorted. Used when appending to an
    /// inline array leaves key parts to wrap the value in.
    Table(Vec<(String, Value)>),
}

/// A TOML document whose layout (comments, whitespace, order) is preserved
/// across edits.
#[derive(Debug, Clone)]
pub struct Document {
    data: String,
    index: Index,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// A pending replacement of a byte range. An empty range is an insertion;
/// an empty replacement is a deletion.
#[derive(Debug, Clone)]
struct Splice {
    span: Span,
    repl: String,
}

fn validate(data: &str) -> Result<toml::Table, Error> {
    toml::from_str::<toml::Table>(data).map_err(|e| Error::new(format!("toml: {}", e.message())))
}

fn path_string(key: &[&str]) -> String {
    key.join(".")
}

impl Document {
    /// Reads a TOML document. Syntactic and semantic errors (duplicate keys,
    /// …) are returned: editing an invalid document is meaningless.
    pub fn parse(src: &str) -> Result<Self, Error> {
        validate(src)?;
        let index = reindex(src).map_err(Error::new)?;
        Ok(Self {
            data: src.to_owned(),
            index,
        })
    }

    /// The decoded value at key, like go-toml's `Unmarshal` into `any`:
    /// tables become tables, arrays and arrays of tables become arrays.
    /// Array elements are addressed by decimal index. An empty path returns
    /// the whole document.
    pub fn get(&self, key: &[&str]) -> Option<toml::Value> {
        let mut v = toml::Value::Table(validate(&self.data).ok()?);
        for k in key {
            v = match v {
                toml::Value::Table(mut t) => t.remove(*k)?,
                toml::Value::Array(mut a) => {
                    let i = parse_index(k)?;
                    if i >= a.len() {
                        return None;
                    }
                    a.swap_remove(i)
                }
                _ => return None,
            };
        }
        Some(v)
    }

    /// Sets the value at key, creating the tables leading to it.
    ///
    /// An existing value has only its bytes replaced. Otherwise a new
    /// `key = value` line is added at the end of the section of the closest
    /// existing parent table, under a new `[table]` header or dotted keys as
    /// appropriate. Setting a path that designates an existing table or array
    /// of tables is an error.
    pub fn set(&mut self, key: &[&str], value: &Value) -> Result<(), Error> {
        if key.is_empty() {
            return Err(Error::new("toml/edit: key must have at least one element"));
        }
        let mut t = 0;
        let mut i = 0;
        while i < key.len() {
            let Some(it) = self.index.tables[t].items.get(key[i]) else {
                break;
            };
            let last = i == key.len() - 1;
            match it {
                Item::Leaf(lf) => {
                    if last {
                        let raw = render_value(value)?;
                        let span = lf.value.clone();
                        return self.apply(vec![Splice { span, repl: raw }]);
                    }
                    let lf = lf.clone();
                    return self.set_inline(&lf, key, i + 1, value);
                }
                Item::Array(arr) => {
                    if last {
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: it is an array of tables; address its elements by index or delete it first",
                            path_string(key)
                        )));
                    }
                    let Some(idx) = parse_index(key[i + 1]) else {
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: {:?} is an array of tables and {:?} is not an index",
                            path_string(key),
                            path_string(&key[..=i]),
                            key[i + 1]
                        )));
                    };
                    if idx > arr.len() {
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: index {idx} out of range: {:?} has {} elements",
                            path_string(key),
                            path_string(&key[..=i]),
                            arr.len()
                        )));
                    }
                    if idx == arr.len() {
                        let arr = arr.clone();
                        return self.append_element(&arr, key, i + 2, value);
                    }
                    if i + 1 == key.len() - 1 {
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: it is a table; delete it first or set its keys individually",
                            path_string(key)
                        )));
                    }
                    t = arr[idx];
                    i += 2;
                }
                Item::Table(c) => {
                    if last {
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: it is a table; delete it first or set its keys individually",
                            path_string(key)
                        )));
                    }
                    t = *c;
                    i += 1;
                }
            }
        }

        // key[i..] does not exist and t is the deepest existing table.
        let suffix = &key[i..];
        if suffix.is_empty() {
            return Err(Error::new(format!(
                "toml/edit: internal error: no keys left to create for {:?}",
                path_string(key)
            )));
        }
        let tb = &self.index.tables[t];

        // Tables defined by dotted keys, and tables inside array-of-tables
        // elements that have no section of their own, are extended with
        // dotted key-values in their nearest section.
        if tb.dotted || (tb.via_array && !tb.explicit) {
            let sec = self.index.nearest_section(t);
            let sec_t = &self.index.tables[sec];
            let mut rel: Vec<&str> = tb.path[sec_t.path.len()..]
                .iter()
                .map(String::as_str)
                .collect();
            rel.extend_from_slice(suffix);
            let line = self.render_dotted_kv(&rel, value)?;
            let at = sec_t.insert_at.unwrap_or(0);
            return self.apply(vec![self.insertion(at, &line, false)]);
        }

        if tb.explicit || tb.root {
            // A [header] below an array element would attach to the last
            // element of the array, not necessarily this one: dotted keys
            // stay unambiguous.
            if suffix.len() == 1 || tb.via_array {
                let line = self.render_dotted_kv(suffix, value)?;
                let at = tb.insert_at.unwrap_or(0);
                return self.apply(vec![self.insertion(at, &line, false)]);
            }
        }

        // The remaining tables need a section of their own, placed right
        // after everything that belongs to t. A single header with the full
        // dotted path creates all of them.
        let mut header_path: Vec<&str> = tb.path.iter().map(String::as_str).collect();
        header_path.extend_from_slice(&suffix[..suffix.len() - 1]);
        let header_key = render_key_path(&header_path);
        let kv = self.render_dotted_kv(&suffix[suffix.len() - 1..], value)?;
        let text = format!("[{header_key}]{}{kv}", self.eol());
        let at = self.index.subtree_end(t);
        self.apply(vec![self.insertion(at, &text, true)])
    }

    /// Appends a new `[[element]]` to an existing array of tables,
    /// initialized with the key-value described by `key[rest_from..]`.
    fn append_element(
        &mut self,
        arr: &[usize],
        key: &[&str],
        rest_from: usize,
        value: &Value,
    ) -> Result<(), Error> {
        let rest = &key[rest_from.min(key.len())..];
        if rest.is_empty() {
            return Err(Error::new(format!(
                "toml/edit: cannot set {:?}: a new array element is a table; set its keys individually",
                path_string(key)
            )));
        }
        let path: Vec<&str> = self.index.tables[arr[0]]
            .path
            .iter()
            .map(String::as_str)
            .collect();
        let header_key = render_key_path(&path);
        let kv = self.render_dotted_kv(rest, value)?;
        let text = format!("[[{header_key}]]{}{kv}", self.eol());
        let at = self.index.array_subtree_end(arr);
        self.apply(vec![self.insertion(at, &text, true)])
    }

    /// Removes the value, key-value, table, or array of tables at key, with
    /// the comments attached to it. Reports whether something was deleted.
    /// Deleting a table removes all its content, including sections defined
    /// elsewhere in the document.
    pub fn delete(&mut self, key: &[&str]) -> bool {
        // Document-level targets are removed in one batch. A target inside an
        // inline value may be expressed by several key-values of a dotted
        // group: those are removed one at a time, re-resolving in between.
        let mut deleted = false;
        loop {
            let Some((splices, again)) = self.resolve_delete(key) else {
                return deleted;
            };
            if self.apply(splices).is_err() {
                return deleted;
            }
            deleted = true;
            if !again {
                return true;
            }
        }
    }

    fn resolve_delete(&self, key: &[&str]) -> Option<(Vec<Splice>, bool)> {
        if key.is_empty() {
            return None;
        }
        let mut t = 0;
        let mut i = 0;
        while i < key.len() {
            let it = self.index.tables[t].items.get(key[i])?;
            let last = i == key.len() - 1;
            match it {
                Item::Leaf(lf) => {
                    if last {
                        return Some((self.deletion_splices(vec![lf.line.clone()]), false));
                    }
                    let (s, again) = self.delete_inline(lf, key, i + 1)?;
                    return Some((vec![s], again));
                }
                Item::Array(arr) => {
                    if last {
                        return Some((self.deletion_splices(self.collect_spans(it)), false));
                    }
                    let idx = parse_index(key[i + 1])?;
                    if idx >= arr.len() {
                        return None;
                    }
                    if i + 1 == key.len() - 1 {
                        let mut spans = Vec::new();
                        self.collect_table_spans(arr[idx], &mut spans);
                        return Some((self.deletion_splices(spans), false));
                    }
                    t = arr[idx];
                    i += 2;
                }
                Item::Table(c) => {
                    if last {
                        return Some((self.deletion_splices(self.collect_spans(it)), false));
                    }
                    t = *c;
                    i += 1;
                }
            }
        }
        None
    }

    fn deletion_splices(&self, spans: Vec<Span>) -> Vec<Splice> {
        merge_spans(spans)
            .into_iter()
            .map(|s| Splice {
                span: self.absorb_trailing_blanks(s),
                repl: String::new(),
            })
            .collect()
    }

    /// The byte ranges expressing an item: key-value lines and whole
    /// sections. Ranges may overlap; merge_spans resolves that.
    fn collect_spans(&self, it: &Item) -> Vec<Span> {
        let mut spans = Vec::new();
        self.collect_item_spans(it, &mut spans);
        spans
    }

    fn collect_item_spans(&self, it: &Item, spans: &mut Vec<Span>) {
        match it {
            Item::Leaf(lf) => spans.push(lf.line.clone()),
            Item::Table(t) => self.collect_table_spans(*t, spans),
            Item::Array(arr) => {
                for e in arr {
                    self.collect_table_spans(*e, spans);
                }
            }
        }
    }

    fn collect_table_spans(&self, t: usize, spans: &mut Vec<Span>) {
        let tb = &self.index.tables[t];
        if tb.explicit {
            spans.push(tb.section.clone());
        }
        for child in tb.items.values() {
            self.collect_item_spans(child, spans);
        }
    }

    /// Replaces the given byte ranges, then validates and re-indexes. If the
    /// result is not a valid TOML document, nothing changes.
    fn apply(&mut self, mut splices: Vec<Splice>) -> Result<(), Error> {
        splices.sort_by_key(|s| s.span.start);
        let mut out = String::with_capacity(self.data.len());
        let mut prev = 0;
        for s in &splices {
            if s.span.start < prev {
                return Err(Error::new("toml/edit: internal error: overlapping edits"));
            }
            out.push_str(&self.data[prev..s.span.start]);
            out.push_str(&s.repl);
            prev = s.span.end;
        }
        out.push_str(&self.data[prev..]);

        validate(&out).map_err(|e| {
            Error::new(format!(
                "toml/edit: edit would produce an invalid TOML document: {e}"
            ))
        })?;
        let index = reindex(&out).map_err(|e| {
            Error::new(format!(
                "toml/edit: edit would produce an invalid TOML document: {e}"
            ))
        })?;
        self.data = out;
        self.index = index;
        Ok(())
    }

    /// The splice inserting text (one or more full lines, ending with a
    /// newline) at pos, a line boundary or the end of the document. Sections
    /// get a blank line separating them from the surrounding expressions.
    fn insertion(&self, pos: usize, text: &str, section: bool) -> Splice {
        let eol = self.eol();
        let bytes = self.data.as_bytes();
        let mut b = String::new();
        let fresh_line = pos == 0 || bytes[pos - 1] == b'\n';
        if !fresh_line {
            // Only possible at the end of a document with no final newline.
            b.push_str(eol);
        }
        if section && pos > 0 && (!fresh_line || !self.blank_line_before(pos)) {
            b.push_str(eol);
        }
        b.push_str(text);
        if section && pos < self.data.len() {
            b.push_str(eol);
        }
        Splice {
            span: pos..pos,
            repl: b,
        }
    }

    /// Extends a deletion reaching the end of the document over the blank
    /// lines that separated it from the content above, so the document does
    /// not end with stray blank lines.
    fn absorb_trailing_blanks(&self, mut s: Span) -> Span {
        if s.end != self.data.len() {
            return s;
        }
        let bytes = self.data.as_bytes();
        while s.start > 0 && bytes[s.start - 1] == b'\n' && self.blank_line_before(s.start) {
            let mut i = s.start - 1;
            if i > 0 && bytes[i - 1] == b'\r' {
                i -= 1;
            }
            s.start = i;
        }
        s
    }

    /// Whether the line ending at pos (excluded) is empty. Only meaningful
    /// when pos is a line boundary.
    fn blank_line_before(&self, pos: usize) -> bool {
        let bytes = self.data.as_bytes();
        let mut i = pos - 1; // bytes[i] == '\n'
        if i > 0 && bytes[i - 1] == b'\r' {
            i -= 1;
        }
        i == 0 || bytes[i - 1] == b'\n'
    }

    /// CRLF if the document already uses it, LF otherwise.
    fn eol(&self) -> &'static str {
        if self.data.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// A `dotted.key = value` line ending with the document's line ending.
    fn render_dotted_kv(&self, parts: &[&str], value: &Value) -> Result<String, Error> {
        Ok(render_inline_kv(parts, value)? + self.eol())
    }
}

impl std::fmt::Display for Document {
    /// The current source. Unmodified, it is identical to the parsed input.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.data)
    }
}

fn merge_spans(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| s.start);
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        if let Some(last) = out.last_mut()
            && s.start <= last.end
        {
            last.end = last.end.max(s.end);
            continue;
        }
        out.push(s);
    }
    out
}

/// The bare TOML representation of a value, in inline form. Raw values are
/// used as-is (trimmed).
fn render_value(value: &Value) -> Result<String, Error> {
    match value {
        Value::Raw(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Err(Error::new("toml/edit: raw value is empty"));
            }
            Ok(raw.to_owned())
        }
        Value::Str(s) => Ok(encode::string(s)),
        Value::Table(entries) => {
            let mut entries: Vec<&(String, Value)> = entries.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = String::from("{");
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&encode::key(k));
                out.push_str(" = ");
                out.push_str(&render_value(v)?);
            }
            out.push('}');
            Ok(out)
        }
    }
}

/// A `dotted.key = value` fragment without a line ending.
fn render_inline_kv(parts: &[&str], value: &Value) -> Result<String, Error> {
    Ok(format!(
        "{} = {}",
        render_key_path(parts),
        render_value(value)?
    ))
}

/// A dotted key path, quoting the parts that need it like the encoder does.
fn render_key_path(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| encode::key(p))
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests;
