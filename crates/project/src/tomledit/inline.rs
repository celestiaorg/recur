//! Editing inside inline values (arrays and inline tables): precise splices
//! that rewrite only the element being edited.

use super::index::{Leaf, line_end, parse_index};
use super::syntax::{Node, Span};
use super::{Document, Error, Splice, Value, path_string, render_inline_kv, render_value};

fn common_prefix(a: &[String], b: &[&str]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == *y).count()
}

impl Document {
    /// Set when the path continues inside the value of lf.
    pub(super) fn set_inline(
        &mut self,
        lf: &Leaf,
        key: &[&str],
        from: usize,
        value: &Value,
    ) -> Result<(), Error> {
        let s = self.inline_set_splice(&lf.node, key, from, value)?;
        self.apply(vec![s])
    }

    fn inline_set_splice(
        &self,
        n: &Node,
        key: &[&str],
        from: usize,
        value: &Value,
    ) -> Result<Splice, Error> {
        match n {
            Node::InlineTable { span, kvs } => {
                let parts = &key[from..];
                for e in kvs {
                    let e_parts = e.parts();
                    let m = common_prefix(&e_parts, parts);
                    if m == e_parts.len() {
                        // The key-value's full key lies on the path.
                        if m == parts.len() {
                            return Ok(Splice {
                                span: e.value.span(),
                                repl: render_value(value)?,
                            });
                        }
                        return self.inline_set_splice(&e.value, key, from + m, value);
                    }
                    if m == parts.len() {
                        // The path stops at a table implied by this dotted key.
                        return Err(Error::new(format!(
                            "toml/edit: cannot set {:?}: it is a table; delete it first or set its keys individually",
                            path_string(key)
                        )));
                    }
                }
                // Not found: add a new key-value to this inline table.
                let text = render_inline_kv(parts, value)?;
                let elems: Vec<Span> = kvs.iter().map(|k| k.span.clone()).collect();
                Ok(self.container_insert(span, &elems, &text))
            }
            Node::Array { span, elems } => {
                let Some(idx) = parse_index(key[from]) else {
                    return Err(Error::new(format!(
                        "toml/edit: cannot set {:?}: {:?} is an array and {:?} is not an index",
                        path_string(key),
                        path_string(&key[..from]),
                        key[from]
                    )));
                };
                if idx > elems.len() {
                    return Err(Error::new(format!(
                        "toml/edit: cannot set {:?}: index {idx} out of range: {:?} has {} elements",
                        path_string(key),
                        path_string(&key[..from]),
                        elems.len()
                    )));
                }
                if idx == elems.len() {
                    // Append; the remaining key parts wrap the value in tables.
                    let mut v = value.clone();
                    for k in key[from + 1..].iter().rev() {
                        v = Value::Table(vec![((*k).to_owned(), v)]);
                    }
                    let spans: Vec<Span> = elems.iter().map(Node::span).collect();
                    return Ok(self.container_insert(span, &spans, &render_value(&v)?));
                }
                if from == key.len() - 1 {
                    return Ok(Splice {
                        span: elems[idx].span(),
                        repl: render_value(value)?,
                    });
                }
                self.inline_set_splice(&elems[idx], key, from + 1, value)
            }
            Node::Scalar(_) => Err(Error::new(format!(
                "toml/edit: cannot set {:?}: {:?} is not a table or array",
                path_string(key),
                path_string(&key[..from])
            ))),
        }
    }

    /// The splice removing the target of key inside the value of lf. A dotted
    /// key group inside an inline table is removed one key-value at a time:
    /// the bool reports that the caller must resolve again for the rest.
    pub(super) fn delete_inline(
        &self,
        lf: &Leaf,
        key: &[&str],
        from: usize,
    ) -> Option<(Splice, bool)> {
        self.inline_delete_splice(&lf.node, key, from)
    }

    fn inline_delete_splice(&self, n: &Node, key: &[&str], from: usize) -> Option<(Splice, bool)> {
        match n {
            Node::InlineTable { span, kvs } => {
                let parts = &key[from..];
                let spans: Vec<Span> = kvs.iter().map(|k| k.span.clone()).collect();
                for (k, e) in kvs.iter().enumerate() {
                    let e_parts = e.parts();
                    let m = common_prefix(&e_parts, parts);
                    if m == e_parts.len() && m == parts.len() {
                        return Some((self.container_delete(span, &spans, k), false));
                    }
                    if m == e_parts.len() {
                        return self.inline_delete_splice(&e.value, key, from + m);
                    }
                    if m == parts.len() {
                        // The path addresses a table implied by this dotted key:
                        // remove the whole key-value, then resolve again for the
                        // other key-values of the group.
                        return Some((self.container_delete(span, &spans, k), true));
                    }
                }
                None
            }
            Node::Array { span, elems } => {
                let idx = parse_index(key[from])?;
                if idx >= elems.len() {
                    return None;
                }
                if from == key.len() - 1 {
                    let spans: Vec<Span> = elems.iter().map(Node::span).collect();
                    return Some((self.container_delete(span, &spans, idx), false));
                }
                self.inline_delete_splice(&elems[idx], key, from + 1)
            }
            Node::Scalar(_) => None,
        }
    }

    /// Advances past whitespace, newlines, comments and (optionally) commas
    /// between the elements of an inline value.
    fn skip_inline_trivia(&self, mut i: usize, skip_comma: bool) -> usize {
        let bytes = self.data.as_bytes();
        while i < bytes.len() {
            match bytes[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                b',' if skip_comma => i += 1,
                b'#' => i = line_end(&self.data, i),
                _ => return i,
            }
        }
        i
    }

    /// The splice adding text as a new element of the inline container at ns
    /// whose current element spans are elems.
    fn container_insert(&self, ns: &Span, elems: &[Span], text: &str) -> Splice {
        let Some(last) = elems.last() else {
            return Splice {
                span: ns.start + 1..ns.start + 1,
                repl: text.to_owned(),
            };
        };
        let e = last.end;
        let p = self.skip_inline_trivia(e, false);
        if self.data.as_bytes().get(p) == Some(&b',') {
            // Reuse the trailing comma as the separator.
            return Splice {
                span: p + 1..p + 1,
                repl: format!(" {text}"),
            };
        }
        Splice {
            span: e..e,
            repl: format!(", {text}"),
        }
    }

    /// The splice removing element k of the inline container at ns, keeping
    /// the separating commas balanced.
    fn container_delete(&self, ns: &Span, elems: &[Span], k: usize) -> Splice {
        let mut s = if elems.len() == 1 {
            ns.start + 1..elems[0].end
        } else if k < elems.len() - 1 {
            return Splice {
                span: elems[k].start..elems[k + 1].start,
                repl: String::new(),
            };
        } else {
            elems[k - 1].end..elems[k].end
        };
        // Absorb the trailing comma, if any.
        let p = self.skip_inline_trivia(s.end, false);
        if self.data.as_bytes().get(p) == Some(&b',') {
            s.end = p + 1;
        }
        Splice {
            span: s,
            repl: String::new(),
        }
    }
}
