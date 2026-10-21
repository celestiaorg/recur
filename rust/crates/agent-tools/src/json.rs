//! Go `encoding/json` decoding semantics for tool arguments and MCP requests.
//!
//! Agents read decode errors verbatim (R2), so this reproduces `json.Unmarshal`
//! and `json.Decoder` (with `DisallowUnknownFields` for [`strict_unmarshal`]):
//! the scanner's syntax errors, case-insensitive field matching, first-error
//! wins, decoding into existing values on duplicate keys, `UnmarshalTypeError`
//! text with Go type names, and `json.RawMessage` keeping the exact input bytes.

use crate::gostr::quote;

const MAX_NESTING_DEPTH: usize = 10000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    BeginValueOrEmpty,
    BeginValue,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU,
    InStringEscU1,
    InStringEscU12,
    InStringEscU123,
    Neg,
    One,
    Zero,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    T,
    Tr,
    Tru,
    F,
    Fa,
    Fal,
    Fals,
    N,
    Nu,
    Nul,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Continue,
    EndObject,
    EndArray,
    End,
    Error,
}

/// Port of Go's `encoding/json` scanner state machine.
struct Scanner {
    step: Step,
    parse: Vec<Parse>,
    end_top: bool,
    err: Option<String>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

impl Scanner {
    fn new() -> Self {
        Scanner {
            step: Step::BeginValue,
            parse: Vec::new(),
            end_top: false,
            err: None,
        }
    }

    fn error(&mut self, c: u8, context: &str) -> Op {
        self.step = Step::Error;
        self.err = Some(format!("invalid character {} {}", quote_char(c), context));
        Op::Error
    }

    fn push(&mut self, c: u8, p: Parse) -> Op {
        self.parse.push(p);
        if self.parse.len() <= MAX_NESTING_DEPTH {
            Op::Continue
        } else {
            self.error(c, "exceeded max depth")
        }
    }

    fn pop(&mut self) {
        self.parse.pop();
        if self.parse.is_empty() {
            self.step = Step::EndTop;
            self.end_top = true;
        } else {
            self.step = Step::EndValue;
        }
    }

    fn eof(&mut self) -> Op {
        if self.err.is_some() {
            return Op::Error;
        }
        if self.end_top {
            return Op::End;
        }
        self.feed(b' ');
        if self.end_top {
            return Op::End;
        }
        if self.err.is_none() {
            self.err = Some("unexpected end of JSON input".into());
        }
        Op::Error
    }

    fn begin_value(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        self.step = match c {
            b'{' => {
                self.step = Step::BeginStringOrEmpty;
                return self.push(c, Parse::ObjectKey);
            }
            b'[' => {
                self.step = Step::BeginValueOrEmpty;
                return self.push(c, Parse::ArrayValue);
            }
            b'"' => Step::InString,
            b'-' => Step::Neg,
            b'0' => Step::Zero,
            b't' => Step::T,
            b'f' => Step::F,
            b'n' => Step::N,
            b'1'..=b'9' => Step::One,
            _ => return self.error(c, "looking for beginning of value"),
        };
        Op::Continue
    }

    fn end_value(&mut self, c: u8) -> Op {
        let Some(&ps) = self.parse.last() else {
            self.step = Step::EndTop;
            self.end_top = true;
            return self.end_top_step(c);
        };
        if is_space(c) {
            self.step = Step::EndValue;
            return Op::Continue;
        }
        match ps {
            Parse::ObjectKey => {
                if c == b':' {
                    *self.parse.last_mut().expect("non-empty") = Parse::ObjectValue;
                    self.step = Step::BeginValue;
                    return Op::Continue;
                }
                self.error(c, "after object key")
            }
            Parse::ObjectValue => {
                if c == b',' {
                    *self.parse.last_mut().expect("non-empty") = Parse::ObjectKey;
                    self.step = Step::BeginString;
                    return Op::Continue;
                }
                if c == b'}' {
                    self.pop();
                    return Op::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            Parse::ArrayValue => {
                if c == b',' {
                    self.step = Step::BeginValue;
                    return Op::Continue;
                }
                if c == b']' {
                    self.pop();
                    return Op::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }

    fn end_top_step(&mut self, c: u8) -> Op {
        if !is_space(c) {
            self.error(c, "after top-level value");
        }
        Op::End
    }

    fn feed(&mut self, c: u8) -> Op {
        use Step::*;
        let hex_err = "in \\u hexadecimal character escape";
        match self.step {
            BeginValueOrEmpty => {
                if is_space(c) {
                    Op::Continue
                } else if c == b']' {
                    self.end_value(c)
                } else {
                    self.begin_value(c)
                }
            }
            BeginValue => self.begin_value(c),
            BeginStringOrEmpty => {
                if is_space(c) {
                    return Op::Continue;
                }
                if c == b'}' {
                    *self.parse.last_mut().expect("non-empty") = Parse::ObjectValue;
                    return self.end_value(c);
                }
                self.begin_string(c)
            }
            BeginString => self.begin_string(c),
            EndValue => self.end_value(c),
            EndTop => self.end_top_step(c),
            InString => {
                if c == b'"' {
                    self.step = EndValue;
                } else if c == b'\\' {
                    self.step = InStringEsc;
                } else if c < 0x20 {
                    return self.error(c, "in string literal");
                }
                Op::Continue
            }
            InStringEsc => match c {
                b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => {
                    self.step = InString;
                    Op::Continue
                }
                b'u' => {
                    self.step = InStringEscU;
                    Op::Continue
                }
                _ => self.error(c, "in string escape code"),
            },
            InStringEscU | InStringEscU1 | InStringEscU12 | InStringEscU123 => {
                if !is_hex(c) {
                    return self.error(c, hex_err);
                }
                self.step = match self.step {
                    InStringEscU => InStringEscU1,
                    InStringEscU1 => InStringEscU12,
                    InStringEscU12 => InStringEscU123,
                    _ => InString,
                };
                Op::Continue
            }
            Neg => match c {
                b'0' => {
                    self.step = Zero;
                    Op::Continue
                }
                b'1'..=b'9' => {
                    self.step = One;
                    Op::Continue
                }
                _ => self.error(c, "in numeric literal"),
            },
            One => {
                if c.is_ascii_digit() {
                    Op::Continue
                } else {
                    self.zero(c)
                }
            }
            Zero => self.zero(c),
            Dot => {
                if c.is_ascii_digit() {
                    self.step = Dot0;
                    Op::Continue
                } else {
                    self.error(c, "after decimal point in numeric literal")
                }
            }
            Dot0 => {
                if c.is_ascii_digit() {
                    Op::Continue
                } else if c == b'e' || c == b'E' {
                    self.step = E;
                    Op::Continue
                } else {
                    self.end_value(c)
                }
            }
            E => {
                if c == b'+' || c == b'-' {
                    self.step = ESign;
                    Op::Continue
                } else {
                    self.e_sign(c)
                }
            }
            ESign => self.e_sign(c),
            E0 => {
                if c.is_ascii_digit() {
                    Op::Continue
                } else {
                    self.end_value(c)
                }
            }
            T => self.literal(c, b'r', Tr, "in literal true (expecting 'r')"),
            Tr => self.literal(c, b'u', Tru, "in literal true (expecting 'u')"),
            Tru => self.literal(c, b'e', EndValue, "in literal true (expecting 'e')"),
            F => self.literal(c, b'a', Fa, "in literal false (expecting 'a')"),
            Fa => self.literal(c, b'l', Fal, "in literal false (expecting 'l')"),
            Fal => self.literal(c, b's', Fals, "in literal false (expecting 's')"),
            Fals => self.literal(c, b'e', EndValue, "in literal false (expecting 'e')"),
            N => self.literal(c, b'u', Nu, "in literal null (expecting 'u')"),
            Nu => self.literal(c, b'l', Nul, "in literal null (expecting 'l')"),
            Nul => self.literal(c, b'l', EndValue, "in literal null (expecting 'l')"),
            Error => Op::Error,
        }
    }

    fn begin_string(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        if c == b'"' {
            self.step = Step::InString;
            return Op::Continue;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn zero(&mut self, c: u8) -> Op {
        if c == b'.' {
            self.step = Step::Dot;
            return Op::Continue;
        }
        if c == b'e' || c == b'E' {
            self.step = Step::E;
            return Op::Continue;
        }
        self.end_value(c)
    }

    fn e_sign(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = Step::E0;
            return Op::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }

    fn literal(&mut self, c: u8, want: u8, next: Step, context: &str) -> Op {
        if c == want {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, context)
    }
}

/// Go `quoteChar`: the byte as a rune (Latin-1), single-quoted.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => r"'\''".into(),
        b'"' => r#"'"'"#.into(),
        _ => {
            let q = quote(&char::from(c).to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

/// Go `checkValid`: the whole input must be exactly one JSON value.
fn check_valid(data: &str) -> Result<(), String> {
    let mut s = Scanner::new();
    for &c in data.as_bytes() {
        if s.feed(c) == Op::Error {
            return Err(s.err.unwrap_or_default());
        }
    }
    if s.eof() == Op::Error {
        return Err(s.err.unwrap_or_default());
    }
    Ok(())
}

/// Go `Decoder.readValue`: the first JSON value of the input (trailing data
/// is left unread, as `Decoder.Decode` does).
fn first_value(data: &str) -> Result<&str, String> {
    let mut s = Scanner::new();
    let bytes = data.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        match s.feed(c) {
            Op::End => return Ok(&data[..i]),
            Op::EndObject | Op::EndArray => {
                if s.end_value(b' ') == Op::End {
                    return Ok(&data[..=i]);
                }
            }
            Op::Error => return Err(s.err.unwrap_or_default()),
            Op::Continue => {}
        }
    }
    if s.feed(b' ') == Op::End {
        return Ok(data);
    }
    if bytes.iter().any(|&c| !is_space(c)) {
        Err("unexpected EOF".into())
    } else {
        Err("EOF".into())
    }
}

/// One parsed JSON value and its exact input text.
struct Node<'a> {
    raw: &'a str,
    v: NodeV<'a>,
}

enum NodeV<'a> {
    Null,
    Bool(bool),
    Num(&'a str),
    Str(String),
    Arr(Vec<Node<'a>>),
    Obj(Vec<(String, Node<'a>)>),
}

/// Parses already-validated JSON.
struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip_space(&mut self) {
        while self.i < self.b.len() && is_space(self.b[self.i]) {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Node<'a> {
        self.skip_space();
        let start = self.i;
        let v = match self.b[self.i] {
            b'{' => {
                self.i += 1;
                let mut fields = Vec::new();
                loop {
                    self.skip_space();
                    if self.b[self.i] == b'}' {
                        self.i += 1;
                        break;
                    }
                    let key = self.string();
                    self.skip_space();
                    self.i += 1; // ':'
                    let val = self.value();
                    fields.push((key, val));
                    self.skip_space();
                    let c = self.b[self.i];
                    self.i += 1;
                    if c == b'}' {
                        break;
                    }
                }
                NodeV::Obj(fields)
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if self.b[self.i] == b']' {
                        self.i += 1;
                        break;
                    }
                    items.push(self.value());
                    self.skip_space();
                    let c = self.b[self.i];
                    self.i += 1;
                    if c == b']' {
                        break;
                    }
                }
                NodeV::Arr(items)
            }
            b'"' => NodeV::Str(self.string()),
            b't' => {
                self.i += 4;
                NodeV::Bool(true)
            }
            b'f' => {
                self.i += 5;
                NodeV::Bool(false)
            }
            b'n' => {
                self.i += 4;
                NodeV::Null
            }
            _ => {
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                NodeV::Num(&self.s[start..self.i])
            }
        };
        Node {
            raw: &self.s[start..self.i],
            v,
        }
    }

    /// Go `unquoteBytes`: lone surrogates become U+FFFD.
    fn string(&mut self) -> String {
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = self.b[self.i];
            match c {
                b'"' => {
                    self.i += 1;
                    return out;
                }
                b'\\' => {
                    let e = self.b[self.i + 1];
                    self.i += 2;
                    match e {
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let r = self.hex4(self.i);
                            self.i += 4;
                            if (0xD800..0xE000).contains(&r) {
                                if r < 0xDC00
                                    && self.b.get(self.i) == Some(&b'\\')
                                    && self.b.get(self.i + 1) == Some(&b'u')
                                {
                                    let r2 = self.hex4(self.i + 2);
                                    if (0xDC00..0xE000).contains(&r2) {
                                        let cp = 0x10000 + ((r - 0xD800) << 10) + (r2 - 0xDC00);
                                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                                        self.i += 6;
                                        continue;
                                    }
                                }
                                out.push('\u{FFFD}');
                            } else {
                                out.push(char::from_u32(r).unwrap_or('\u{FFFD}'));
                            }
                        }
                        other => out.push(char::from(other)),
                    }
                }
                _ => {
                    let rest = &self.s[self.i..];
                    let end = rest.find(['"', '\\']).unwrap_or(rest.len());
                    out.push_str(&rest[..end]);
                    self.i += end;
                }
            }
        }
    }

    fn hex4(&self, at: usize) -> u32 {
        self.s
            .get(at..at + 4)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(0xFFFD)
    }
}

fn parse(valid: &str) -> Node<'_> {
    Parser {
        s: valid,
        b: valid.as_bytes(),
        i: 0,
    }
    .value()
}

/// The Go type a decoded value is assigned to.
#[derive(Debug)]
pub enum Ty {
    /// `string`
    Str,
    /// `int`
    Int,
    /// `float64`
    Float,
    /// `bool`
    Bool,
    /// `*bool`: distinguishes omitted (or null) from false.
    OptBool,
    /// `*string`: distinguishes omitted (or null) from empty.
    OptStr,
    /// `json.RawMessage`
    Raw,
    /// `[]T`
    List(&'static Ty),
    /// A struct.
    Struct(&'static Shape),
}

/// One struct field: its JSON tag name and Go declaration.
#[derive(Debug)]
pub struct Field {
    pub json: &'static str,
    pub go: &'static str,
    pub ty: Ty,
}

/// A Go struct type. `name` is `reflect.Type.Name()` (empty for anonymous
/// structs); `qualified` is its `String()` when named (`agenttools.TodoItem`).
#[derive(Debug)]
pub struct Shape {
    pub name: &'static str,
    pub qualified: &'static str,
    pub fields: &'static [Field],
}

impl Shape {
    pub const fn anon(fields: &'static [Field]) -> Shape {
        Shape {
            name: "",
            qualified: "",
            fields,
        }
    }
}

/// Shorthand for a [`Field`].
pub const fn field(json: &'static str, go: &'static str, ty: Ty) -> Field {
    Field { json, go, ty }
}

fn ty_string(ty: &Ty) -> String {
    match ty {
        Ty::Str => "string".into(),
        Ty::Int => "int".into(),
        Ty::Float => "float64".into(),
        Ty::Bool => "bool".into(),
        Ty::OptBool => "*bool".into(),
        Ty::OptStr => "*string".into(),
        Ty::Raw => "json.RawMessage".into(),
        Ty::List(elem) => format!("[]{}", ty_string(elem)),
        Ty::Struct(shape) => {
            if !shape.qualified.is_empty() {
                return shape.qualified.into();
            }
            if shape.fields.is_empty() {
                return "struct {}".into();
            }
            let fields: Vec<String> = shape
                .fields
                .iter()
                .map(|f| {
                    let tag = format!("json:\"{}\"", f.json);
                    format!("{} {} {}", f.go, ty_string(&f.ty), quote(&tag))
                })
                .collect();
            format!("struct {{ {} }}", fields.join("; "))
        }
    }
}

/// A decoded Go value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    OptBool(Option<bool>),
    OptStr(Option<String>),
    /// Exact input text; empty when the field was absent (nil RawMessage).
    Raw(String),
    List(Vec<Val>),
    Struct(Vec<Val>),
}

fn zero(ty: &Ty) -> Val {
    match ty {
        Ty::Str => Val::Str(String::new()),
        Ty::Int => Val::Int(0),
        Ty::Float => Val::Float(0.0),
        Ty::Bool => Val::Bool(false),
        Ty::OptBool => Val::OptBool(None),
        Ty::OptStr => Val::OptStr(None),
        Ty::Raw => Val::Raw(String::new()),
        Ty::List(_) => Val::List(Vec::new()),
        Ty::Struct(shape) => Val::Struct(shape.fields.iter().map(|f| zero(&f.ty)).collect()),
    }
}

/// Go's error context: the struct whose field is being decoded and the
/// field path from the root.
#[derive(Clone, Default)]
struct Ctx {
    in_struct: Option<&'static str>,
    path: Vec<&'static str>,
}

struct Decoder {
    strict: bool,
    err: Option<String>,
}

impl Decoder {
    fn save(&mut self, err: String) {
        if self.err.is_none() {
            self.err = Some(err);
        }
    }

    fn type_error(&mut self, value: &str, ty: &Ty, cx: &Ctx) {
        // Go reports the pointer's element type.
        let ty = match ty {
            Ty::OptBool => "bool".into(),
            Ty::OptStr => "string".into(),
            other => ty_string(other),
        };
        let msg = match cx.in_struct {
            Some(name) => format!(
                "json: cannot unmarshal {value} into Go struct field {name}.{} of type {ty}",
                cx.path.join(".")
            ),
            None => format!("json: cannot unmarshal {value} into Go value of type {ty}"),
        };
        self.save(msg);
    }

    fn value(&mut self, node: &Node<'_>, ty: &Ty, dst: &mut Val, cx: &Ctx) {
        if let Ty::Raw = ty {
            *dst = Val::Raw(node.raw.to_string());
            return;
        }
        match &node.v {
            NodeV::Obj(fields) => self.object(fields, ty, dst, cx),
            NodeV::Arr(items) => self.array(items, ty, dst, cx),
            _ => self.literal(node, ty, dst, cx),
        }
    }

    fn object(&mut self, fields: &[(String, Node<'_>)], ty: &Ty, dst: &mut Val, cx: &Ctx) {
        let Ty::Struct(shape) = ty else {
            self.type_error("object", ty, cx);
            return;
        };
        let Val::Struct(vals) = dst else { return };
        for (key, node) in fields {
            let found = shape.fields.iter().position(|f| f.json == key).or_else(|| {
                let folded = fold_name(key);
                shape
                    .fields
                    .iter()
                    .position(|f| fold_name(f.json) == folded)
            });
            match found {
                Some(i) => {
                    let f = &shape.fields[i];
                    let mut inner = cx.clone();
                    inner.in_struct = Some(shape.name);
                    inner.path.push(f.json);
                    self.value(node, &f.ty, &mut vals[i], &inner);
                }
                None if self.strict => self.save(format!("json: unknown field {}", quote(key))),
                None => {}
            }
        }
    }

    fn array(&mut self, items: &[Node<'_>], ty: &Ty, dst: &mut Val, cx: &Ctx) {
        let (Ty::List(elem), Val::List(vals)) = (ty, &mut *dst) else {
            self.type_error("array", ty, cx);
            return;
        };
        for (i, node) in items.iter().enumerate() {
            if i >= vals.len() {
                vals.push(zero(elem));
            }
            self.value(node, elem, &mut vals[i], cx);
        }
        vals.truncate(items.len());
    }

    fn literal(&mut self, node: &Node<'_>, ty: &Ty, dst: &mut Val, cx: &Ctx) {
        if let (NodeV::Null, Ty::OptBool | Ty::OptStr | Ty::List(_)) = (&node.v, ty) {
            *dst = zero(ty);
            return;
        }
        if let NodeV::Null = node.v {
            return;
        }
        // A non-null literal allocates the pointer before assigning (Go indirect).
        match (ty, &*dst) {
            (Ty::OptBool, Val::OptBool(None)) => *dst = Val::OptBool(Some(false)),
            (Ty::OptStr, Val::OptStr(None)) => *dst = Val::OptStr(Some(String::new())),
            _ => {}
        }
        match (&node.v, ty, dst) {
            (NodeV::Bool(b), Ty::Bool, Val::Bool(v)) => *v = *b,
            (NodeV::Bool(b), Ty::OptBool, Val::OptBool(v)) => *v = Some(*b),
            (NodeV::Bool(_), _, _) => self.type_error("bool", ty, cx),
            (NodeV::Str(s), Ty::Str, Val::Str(v)) => *v = s.clone(),
            (NodeV::Str(s), Ty::OptStr, Val::OptStr(v)) => *v = Some(s.clone()),
            (NodeV::Str(_), _, _) => self.type_error("string", ty, cx),
            (NodeV::Num(n), Ty::Int, Val::Int(v)) => match n.parse::<i64>() {
                Ok(i) => *v = i,
                Err(_) => self.type_error(&format!("number {n}"), ty, cx),
            },
            (NodeV::Num(n), Ty::Float, Val::Float(v)) => match n.parse::<f64>() {
                Ok(f) if f.is_finite() => *v = f,
                _ => self.type_error(&format!("number {n}"), ty, cx),
            },
            (NodeV::Num(_), _, _) => self.type_error("number", ty, cx),
            _ => {}
        }
    }
}

/// Go `foldName` for matching against ASCII field names: ASCII upper-cased,
/// U+017F (ſ) and U+212A (Kelvin) fold to S and K like `unicode.SimpleFold`.
fn fold_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => c.to_ascii_uppercase(),
            '\u{17F}' => 'S',
            '\u{212A}' => 'K',
            _ => c,
        })
        .collect()
}

fn decode(valid: &str, ty: &Ty, strict: bool) -> Result<Val, String> {
    let node = parse(valid);
    let mut val = zero(ty);
    let mut d = Decoder { strict, err: None };
    d.value(&node, ty, &mut val, &Ctx::default());
    match d.err {
        Some(e) => Err(e),
        None => Ok(val),
    }
}

/// Go `json.Unmarshal(data, &v)` into a value of type `ty`.
pub fn unmarshal_ty(data: &str, ty: &Ty) -> Result<Val, String> {
    check_valid(data)?;
    decode(data, ty, false)
}

/// Go `json.Unmarshal` into a struct.
pub fn unmarshal(data: &str, shape: &'static Shape) -> Result<Obj, String> {
    unmarshal_ty(data, &Ty::Struct(shape)).map(|v| Obj::new(shape, v))
}

/// `_ = json.Unmarshal(data, &v)`: whatever Go leaves in `v`. Invalid JSON
/// decodes nothing; type errors keep the fields decoded around them.
pub fn unmarshal_ignoring_errors(data: &str, shape: &'static Shape) -> Obj {
    let ty = Ty::Struct(shape);
    let mut val = zero(&ty);
    if check_valid(data).is_ok() {
        let mut d = Decoder {
            strict: false,
            err: None,
        };
        d.value(&parse(data), &ty, &mut val, &Ctx::default());
    }
    Obj::new(shape, val)
}

/// agenttools `strictUnmarshal`: empty args decode to the zero value; otherwise
/// a `json.Decoder` with `DisallowUnknownFields`, errors prefixed
/// `invalid arguments: `.
pub fn strict_unmarshal(data: &str, shape: &'static Shape) -> Result<Obj, String> {
    if data.is_empty() {
        return Ok(Obj::new(shape, zero(&Ty::Struct(shape))));
    }
    first_value(data)
        .and_then(|v| decode(v, &Ty::Struct(shape), true))
        .map(|v| Obj::new(shape, v))
        .map_err(|e| format!("invalid arguments: {e}"))
}

/// A decoded struct, read by JSON field name.
#[derive(Debug, Clone)]
pub struct Obj {
    shape: &'static Shape,
    vals: Vec<Val>,
}

impl Obj {
    fn new(shape: &'static Shape, v: Val) -> Obj {
        let Val::Struct(vals) = v else {
            unreachable!("struct shapes decode to Val::Struct")
        };
        Obj { shape, vals }
    }

    fn get(&self, name: &str) -> &Val {
        let i = self
            .shape
            .fields
            .iter()
            .position(|f| f.json == name)
            .unwrap_or_else(|| panic!("field {name} is not in the shape"));
        &self.vals[i]
    }

    pub fn str(&self, name: &str) -> &str {
        match self.get(name) {
            Val::Str(s) | Val::Raw(s) => s,
            _ => "",
        }
    }

    pub fn int(&self, name: &str) -> i64 {
        match self.get(name) {
            Val::Int(i) => *i,
            _ => 0,
        }
    }

    pub fn float(&self, name: &str) -> f64 {
        match self.get(name) {
            Val::Float(f) => *f,
            _ => 0.0,
        }
    }

    pub fn bool(&self, name: &str) -> bool {
        matches!(self.get(name), Val::Bool(true))
    }

    pub fn opt_str(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Val::OptStr(s) => s.as_deref(),
            _ => None,
        }
    }

    pub fn opt_bool(&self, name: &str) -> Option<bool> {
        match self.get(name) {
            Val::OptBool(b) => *b,
            _ => None,
        }
    }

    /// A `[]string` or `[]json.RawMessage` field.
    pub fn strs(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Val::List(items) => items
                .iter()
                .map(|v| match v {
                    Val::Str(s) | Val::Raw(s) => s.clone(),
                    _ => String::new(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A `[]struct` field.
    pub fn objs(&self, name: &str) -> Vec<Obj> {
        let i = self
            .shape
            .fields
            .iter()
            .position(|f| f.json == name)
            .unwrap_or_else(|| panic!("field {name} is not in the shape"));
        let (Ty::List(Ty::Struct(shape)), Val::List(items)) =
            (&self.shape.fields[i].ty, &self.vals[i])
        else {
            return Vec::new();
        };
        items.iter().map(|v| Obj::new(shape, v.clone())).collect()
    }
}

/// Go `json.Valid`.
pub fn valid(data: &str) -> bool {
    check_valid(data).is_ok()
}

/// Go `json.Compact`, with `json.Marshal`'s HTML escaping, as applied to a
/// `json.RawMessage` being encoded. Errors on invalid JSON like Go does.
pub fn compact(data: &str) -> Result<String, String> {
    check_valid(data)?;
    let mut out = String::with_capacity(data.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in data.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if matches!(c, ' ' | '\t' | '\n' | '\r') {
            continue;
        }
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    static TODO: Shape = Shape {
        name: "TodoItem",
        qualified: "agenttools.TodoItem",
        fields: &[
            field("id", "ID", Ty::Str),
            field("content", "Content", Ty::Str),
        ],
    };
    static OPTS: Shape =
        Shape::anon(&[field("text", "Text", Ty::Str), field("tag", "Tag", Ty::Str)]);
    static IN: Shape = Shape::anon(&[
        field("file", "File", Ty::Str),
        field("line", "Line", Ty::Int),
        field("passed", "Passed", Ty::OptBool),
        field("input", "Input", Ty::Raw),
        field("options", "Options", Ty::List(&Ty::Struct(&OPTS))),
        field("todos", "Todos", Ty::List(&Ty::Struct(&TODO))),
        field("ids", "IDs", Ty::List(&Ty::Str)),
    ]);

    #[test]
    fn decodes_like_go() {
        let o = unmarshal(
            r#"{"FILE":"a","line":3,"passed":true,"input":{ "a" : 1 },"ids":["x"]}"#,
            &IN,
        )
        .unwrap();
        assert_eq!(o.str("file"), "a");
        assert_eq!(o.int("line"), 3);
        assert_eq!(o.opt_bool("passed"), Some(true));
        assert_eq!(o.str("input"), r#"{ "a" : 1 }"#);
        assert_eq!(o.strs("ids"), vec!["x"]);
        let o = unmarshal(r#"{"input":null,"passed":null}"#, &IN).unwrap();
        assert_eq!(o.str("input"), "null");
        assert_eq!(o.opt_bool("passed"), None);
    }

    #[test]
    fn duplicate_keys_decode_into_existing_values() {
        let o = unmarshal(
            r#"{"options":[{"text":"a","tag":"t"}],"options":[{"text":"b"}]}"#,
            &IN,
        )
        .unwrap();
        let opts = o.objs("options");
        assert_eq!((opts[0].str("text"), opts[0].str("tag")), ("b", "t"));
    }

    #[test]
    fn type_errors_name_go_types() {
        assert_eq!(
            unmarshal(r#"{"todos":[{"id":5}]}"#, &IN).unwrap_err(),
            "json: cannot unmarshal number into Go struct field TodoItem.todos.id of type string"
        );
        assert_eq!(
            unmarshal(r#"{"options":["x"]}"#, &IN).unwrap_err(),
            "json: cannot unmarshal string into Go struct field .options of type struct { Text string \"json:\\\"text\\\"\"; Tag string \"json:\\\"tag\\\"\" }"
        );
        assert_eq!(
            unmarshal(r#"{"passed":3}"#, &IN).unwrap_err(),
            "json: cannot unmarshal number into Go struct field .passed of type bool"
        );
        assert_eq!(
            unmarshal(r#"{"line":1e400}"#, &IN).unwrap_err(),
            "json: cannot unmarshal number 1e400 into Go struct field .line of type int"
        );
    }

    #[test]
    fn syntax_errors_match_go() {
        assert_eq!(
            unmarshal("", &IN).unwrap_err(),
            "unexpected end of JSON input"
        );
        assert_eq!(
            unmarshal("{} x", &IN).unwrap_err(),
            "invalid character 'x' after top-level value"
        );
        assert_eq!(
            strict_unmarshal(" ", &IN).unwrap_err(),
            "invalid arguments: EOF"
        );
        assert_eq!(
            strict_unmarshal("{", &IN).unwrap_err(),
            "invalid arguments: unexpected EOF"
        );
        assert!(strict_unmarshal(r#"{"file":"a"} trailing"#, &IN).is_ok());
        assert_eq!(quote_char(0xC3), "'Ã'");
        assert_eq!(quote_char(0xA0), "'\\u00a0'");
        assert_eq!(quote_char(b'\t'), r"'\t'");
    }

    #[test]
    fn compacts_and_escapes() {
        assert_eq!(
            compact("{ \"a\" : [1, 2], \"s\": \"x y<\" }").unwrap(),
            "{\"a\":[1,2],\"s\":\"x y\\u003c\"}"
        );
        assert!(compact("{").is_err());
    }
}
