//! Top-level TOML expressions with byte spans, built from `toml_parser`
//! events. This plays the role of go-toml's `unstable.Parser` with
//! `KeepComments`: full-line comments are their own expressions, while a
//! comment trailing a key-value or header belongs to that line.

use std::ops::Range;

use toml_parser::Source;
use toml_parser::parser::{Event, EventKind, parse_document};

pub(crate) type Span = Range<usize>;

#[derive(Debug, Clone)]
pub(crate) struct KeyPart {
    pub name: String,
    pub span: Span,
}

/// A value with its exact byte span (from the first byte of a scalar or the
/// opening bracket to the last byte or closing bracket).
#[derive(Debug, Clone)]
pub(crate) enum Node {
    Scalar(Span),
    Array { span: Span, elems: Vec<Node> },
    InlineTable { span: Span, kvs: Vec<InlineKv> },
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Scalar(s) => s.clone(),
            Node::Array { span, .. } | Node::InlineTable { span, .. } => span.clone(),
        }
    }
}

/// A key-value inside an inline table; `span` runs from the first key byte
/// to the end of the value.
#[derive(Debug, Clone)]
pub(crate) struct InlineKv {
    pub keys: Vec<KeyPart>,
    pub value: Node,
    pub span: Span,
}

impl InlineKv {
    pub fn parts(&self) -> Vec<String> {
        self.keys.iter().map(|k| k.name.clone()).collect()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    /// A full-line comment, from `#` to the end of its text.
    Comment(Span),
    /// `span` runs from the first key byte to the end of the value.
    KeyValue {
        keys: Vec<KeyPart>,
        value: Node,
        span: Span,
    },
    /// `span` runs from `[` to `]`.
    Table { keys: Vec<KeyPart>, span: Span },
    /// `span` runs from `[[` to `]]`.
    ArrayTable { keys: Vec<KeyPart>, span: Span },
}

/// Splits a document that already parsed as valid TOML into expressions.
pub(crate) fn expressions(src: &str) -> Vec<Expr> {
    let source = Source::new(src);
    let tokens = source.lex().into_vec();
    let mut events: Vec<Event> = Vec::with_capacity(tokens.len());
    parse_document(&tokens, &mut events, &mut ());
    let mut p = Parser {
        source,
        events: &events,
        i: 0,
    };
    p.document()
}

struct Parser<'a, 's> {
    source: Source<'s>,
    events: &'a [Event],
    i: usize,
}

impl<'a> Parser<'a, '_> {
    fn peek(&self) -> Option<&'a Event> {
        self.events.get(self.i)
    }

    fn kind(&self) -> Option<EventKind> {
        self.peek().map(|e| e.kind())
    }

    fn span_of(e: &Event) -> Span {
        e.span().start()..e.span().end()
    }

    fn document(&mut self) -> Vec<Expr> {
        let mut out = Vec::new();
        let mut line_has_expr = false;
        while let Some(e) = self.peek() {
            match e.kind() {
                EventKind::Newline => {
                    line_has_expr = false;
                    self.i += 1;
                }
                EventKind::Comment => {
                    if !line_has_expr {
                        out.push(Expr::Comment(Self::span_of(e)));
                    }
                    self.i += 1;
                }
                EventKind::StdTableOpen => {
                    let (keys, span) = self.header(EventKind::StdTableClose);
                    out.push(Expr::Table { keys, span });
                    line_has_expr = true;
                }
                EventKind::ArrayTableOpen => {
                    let (keys, span) = self.header(EventKind::ArrayTableClose);
                    out.push(Expr::ArrayTable { keys, span });
                    line_has_expr = true;
                }
                EventKind::SimpleKey => {
                    let (keys, value, span) = self.keyval();
                    out.push(Expr::KeyValue { keys, value, span });
                    line_has_expr = true;
                }
                _ => self.i += 1,
            }
        }
        out
    }

    fn header(&mut self, close: EventKind) -> (Vec<KeyPart>, Span) {
        let start = self.peek().map_or(0, |e| e.span().start());
        self.i += 1;
        let mut keys = Vec::new();
        let mut end = start;
        while let Some(e) = self.peek() {
            let k = e.kind();
            if k == EventKind::SimpleKey {
                keys.push(self.key_part(e));
            }
            self.i += 1;
            if k == close {
                end = e.span().end();
                break;
            }
        }
        (keys, start..end)
    }

    fn key_part(&self, e: &Event) -> KeyPart {
        let mut name = String::new();
        if let Some(raw) = self.source.get(e) {
            raw.decode_key(&mut name, &mut ());
        }
        KeyPart {
            name,
            span: Self::span_of(e),
        }
    }

    /// Reads `key(.key)* = value` starting at a SimpleKey event.
    fn keyval(&mut self) -> (Vec<KeyPart>, Node, Span) {
        let mut keys = Vec::new();
        while let Some(e) = self.peek() {
            match e.kind() {
                EventKind::SimpleKey => keys.push(self.key_part(e)),
                EventKind::KeySep | EventKind::Whitespace => {}
                EventKind::KeyValSep => {
                    self.i += 1;
                    break;
                }
                _ => break,
            }
            self.i += 1;
        }
        let value = self.value();
        let start = keys.first().map_or(0, |k| k.span.start);
        let end = value.span().end;
        (keys, value, start..end)
    }

    fn skip_trivia(&mut self) {
        while let Some(k) = self.kind() {
            match k {
                EventKind::Whitespace
                | EventKind::Newline
                | EventKind::Comment
                | EventKind::ValueSep => self.i += 1,
                _ => return,
            }
        }
    }

    fn value(&mut self) -> Node {
        self.skip_trivia();
        let Some(e) = self.peek() else {
            return Node::Scalar(0..0);
        };
        let start = e.span().start();
        match e.kind() {
            EventKind::ArrayOpen => {
                self.i += 1;
                let mut elems = Vec::new();
                loop {
                    self.skip_trivia();
                    match self.peek() {
                        Some(e) if e.kind() == EventKind::ArrayClose => {
                            let end = e.span().end();
                            self.i += 1;
                            return Node::Array {
                                span: start..end,
                                elems,
                            };
                        }
                        Some(_) => elems.push(self.value()),
                        None => {
                            return Node::Array {
                                span: start..self.source.input().len(),
                                elems,
                            };
                        }
                    }
                }
            }
            EventKind::InlineTableOpen => {
                self.i += 1;
                let mut kvs = Vec::new();
                loop {
                    self.skip_trivia();
                    match self.peek() {
                        Some(e) if e.kind() == EventKind::InlineTableClose => {
                            let end = e.span().end();
                            self.i += 1;
                            return Node::InlineTable {
                                span: start..end,
                                kvs,
                            };
                        }
                        Some(e) if e.kind() == EventKind::SimpleKey => {
                            let (keys, value, span) = self.keyval();
                            kvs.push(InlineKv { keys, value, span });
                        }
                        Some(_) => self.i += 1,
                        None => {
                            return Node::InlineTable {
                                span: start..self.source.input().len(),
                                kvs,
                            };
                        }
                    }
                }
            }
            _ => {
                self.i += 1;
                Node::Scalar(Self::span_of(e))
            }
        }
    }
}
