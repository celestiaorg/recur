//! Decoding of tool arguments and MCP requests into runtime [`Shape`]s.
//!
//! Absent fields and `null` decode to the zero value, keys match exactly or
//! else ASCII case-insensitively, and the first type error wins.
//! [`strict_unmarshal`] also rejects unknown fields.

use std::borrow::Cow;

use serde_json::{Map, Value};

/// The type a decoded value is assigned to.
#[derive(Debug)]
pub enum Ty {
    Str,
    Int,
    Float,
    Bool,
    /// Distinguishes omitted (or null) from false.
    OptBool,
    /// Distinguishes omitted (or null) from empty.
    OptStr,
    /// Any JSON value, kept as compact JSON text.
    Raw,
    List(&'static Ty),
    Struct(&'static Shape),
}

/// One struct field by its JSON name.
#[derive(Debug)]
pub struct Field {
    pub json: &'static str,
    pub ty: Ty,
}

#[derive(Debug)]
pub struct Shape {
    pub fields: &'static [Field],
}

impl Shape {
    pub const fn anon(fields: &'static [Field]) -> Shape {
        Shape { fields }
    }
}

/// Shorthand for a [`Field`].
pub const fn field(json: &'static str, ty: Ty) -> Field {
    Field { json, ty }
}

/// A decoded value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    OptBool(Option<bool>),
    OptStr(Option<String>),
    /// Empty when the field was absent.
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

fn ty_name(ty: &Ty) -> &'static str {
    match ty {
        Ty::Str | Ty::OptStr => "string",
        Ty::Int => "integer",
        Ty::Float => "number",
        Ty::Bool | Ty::OptBool => "boolean",
        Ty::Raw => "JSON value",
        Ty::List(_) => "array",
        Ty::Struct(_) => "object",
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

struct Decoder {
    strict: bool,
    err: Option<String>,
}

impl Decoder {
    fn fail(&mut self, msg: String) {
        self.err.get_or_insert(msg);
    }

    fn value(&mut self, v: &Value, ty: &Ty, path: &str) -> Val {
        let got = match (ty, v) {
            (Ty::Raw, v) => Some(Val::Raw(v.to_string())),
            (_, Value::Null) => return zero(ty),
            (Ty::Str, Value::String(s)) => Some(Val::Str(s.clone())),
            (Ty::OptStr, Value::String(s)) => Some(Val::OptStr(Some(s.clone()))),
            (Ty::Bool, Value::Bool(b)) => Some(Val::Bool(*b)),
            (Ty::OptBool, Value::Bool(b)) => Some(Val::OptBool(Some(*b))),
            (Ty::Int, Value::Number(n)) => n.as_i64().map(Val::Int),
            (Ty::Float, Value::Number(n)) => n.as_f64().map(Val::Float),
            (Ty::List(elem), Value::Array(items)) => Some(Val::List(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| self.value(v, elem, &format!("{path}[{i}]")))
                    .collect(),
            )),
            (Ty::Struct(shape), Value::Object(m)) => Some(self.object(m, shape, path)),
            _ => None,
        };
        got.unwrap_or_else(|| {
            let want = ty_name(ty);
            let got = kind(v);
            self.fail(if path.is_empty() {
                format!("expected {want}, got {got}")
            } else {
                format!("{path}: expected {want}, got {got}")
            });
            zero(ty)
        })
    }

    fn object(&mut self, m: &Map<String, Value>, shape: &Shape, path: &str) -> Val {
        let mut vals: Vec<Val> = shape.fields.iter().map(|f| zero(&f.ty)).collect();
        for (key, v) in m {
            let found = shape.fields.iter().position(|f| f.json == key).or_else(|| {
                shape
                    .fields
                    .iter()
                    .position(|f| f.json.eq_ignore_ascii_case(key))
            });
            match found {
                Some(i) => {
                    let f = &shape.fields[i];
                    vals[i] = self.value(v, &f.ty, &join(path, f.json));
                }
                None if self.strict => self.fail(format!("unknown field {:?}", join(path, key))),
                None => {}
            }
        }
        Val::Struct(vals)
    }
}

/// `data` with lone UTF-16 surrogate escapes replaced by `\ufffd`.
/// JavaScript clients can emit them, and serde_json rejects them.
fn repair_surrogates(data: &str) -> Cow<'_, str> {
    if !data.contains("\\ud") && !data.contains("\\uD") {
        return Cow::Borrowed(data);
    }
    let unit = |i: usize| {
        let hex = data.get(i..i + 6)?.strip_prefix("\\u")?;
        u16::from_str_radix(hex, 16).ok()
    };
    let mut out = String::with_capacity(data.len());
    let mut i = 0;
    while let Some(off) = data[i..].find('\\') {
        let at = i + off;
        out.push_str(&data[i..at]);
        match unit(at) {
            Some(0xD800..=0xDBFF) if matches!(unit(at + 6), Some(0xDC00..=0xDFFF)) => {
                out.push_str(&data[at..at + 12]);
                i = at + 12;
            }
            Some(0xD800..=0xDFFF) => {
                out.push_str("\\ufffd");
                i = at + 6;
            }
            _ => {
                let end = at + 1 + data[at + 1..].chars().next().map_or(0, char::len_utf8);
                out.push_str(&data[at..end]);
                i = end;
            }
        }
    }
    out.push_str(&data[i..]);
    Cow::Owned(out)
}

fn decode(data: &str, ty: &Ty, strict: bool) -> Result<Val, String> {
    let v: Value = serde_json::from_str(&repair_surrogates(data)).map_err(|e| e.to_string())?;
    let mut d = Decoder { strict, err: None };
    let val = d.value(&v, ty, "");
    d.err.map_or(Ok(val), Err)
}

/// Decodes `data` as a value of type `ty`.
pub fn unmarshal_ty(data: &str, ty: &Ty) -> Result<Val, String> {
    decode(data, ty, false)
}

/// Decodes `data` as a struct, ignoring unknown fields.
pub fn unmarshal(data: &str, shape: &'static Shape) -> Result<Obj, String> {
    unmarshal_ty(data, &Ty::Struct(shape)).map(|v| Obj::new(shape, v))
}

/// Best-effort decode: invalid JSON yields the zero struct; a type error
/// zeroes only its own field.
pub fn unmarshal_ignoring_errors(data: &str, shape: &'static Shape) -> Obj {
    let v = serde_json::from_str(&repair_surrogates(data)).unwrap_or(Value::Null);
    let mut d = Decoder {
        strict: false,
        err: None,
    };
    Obj::new(shape, d.value(&v, &Ty::Struct(shape), ""))
}

/// Tool arguments: empty args decode to the zero value; unknown fields are
/// rejected; errors are prefixed `invalid arguments: `.
pub fn strict_unmarshal(data: &str, shape: &'static Shape) -> Result<Obj, String> {
    if data.is_empty() {
        return Ok(Obj::new(shape, zero(&Ty::Struct(shape))));
    }
    decode(data, &Ty::Struct(shape), true)
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

/// `data` re-encoded as compact JSON. Errors on invalid JSON.
pub fn compact(data: &str) -> Result<String, String> {
    serde_json::from_str::<Value>(data)
        .map(|v| v.to_string())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TODO: Shape = Shape::anon(&[field("id", Ty::Str), field("content", Ty::Str)]);
    static OPTS: Shape = Shape::anon(&[field("text", Ty::Str), field("tag", Ty::Str)]);
    static IN: Shape = Shape::anon(&[
        field("file", Ty::Str),
        field("line", Ty::Int),
        field("passed", Ty::OptBool),
        field("input", Ty::Raw),
        field("options", Ty::List(&Ty::Struct(&OPTS))),
        field("todos", Ty::List(&Ty::Struct(&TODO))),
        field("ids", Ty::List(&Ty::Str)),
    ]);

    #[test]
    fn decodes_fields() {
        let o = unmarshal(
            r#"{"FILE":"a","line":3,"passed":true,"input":{ "b" : 1, "a": 2 },"ids":["x"],"extra":1}"#,
            &IN,
        )
        .unwrap();
        assert_eq!(o.str("file"), "a");
        assert_eq!(o.int("line"), 3);
        assert_eq!(o.opt_bool("passed"), Some(true));
        assert_eq!(o.str("input"), r#"{"b":1,"a":2}"#);
        assert_eq!(o.strs("ids"), vec!["x"]);
        let o = unmarshal(r#"{"input":null,"passed":null,"line":null}"#, &IN).unwrap();
        assert_eq!(o.str("input"), "null");
        assert_eq!(o.opt_bool("passed"), None);
        assert_eq!(o.int("line"), 0);
        assert_eq!(unmarshal("{}", &IN).unwrap().str("input"), "");
    }

    #[test]
    fn type_errors_name_the_field() {
        assert_eq!(
            unmarshal(r#"{"todos":[{"id":5}]}"#, &IN).unwrap_err(),
            "todos[0].id: expected string, got integer"
        );
        assert_eq!(
            unmarshal(r#"{"options":["x"]}"#, &IN).unwrap_err(),
            "options[0]: expected object, got string"
        );
        assert_eq!(
            unmarshal(r#"{"passed":3}"#, &IN).unwrap_err(),
            "passed: expected boolean, got integer"
        );
        assert_eq!(
            unmarshal(r#"{"line":1.5}"#, &IN).unwrap_err(),
            "line: expected integer, got number"
        );
        assert_eq!(
            unmarshal("[]", &IN).unwrap_err(),
            "expected object, got array"
        );
    }

    #[test]
    fn strict_rejects_unknown_and_invalid() {
        assert_eq!(
            strict_unmarshal(r#"{"file":"a","todos":[{"idx":1}]}"#, &IN).unwrap_err(),
            r#"invalid arguments: unknown field "todos[0].idx""#
        );
        assert!(
            strict_unmarshal("{", &IN)
                .unwrap_err()
                .starts_with("invalid arguments: ")
        );
        assert!(strict_unmarshal(r#"{"file":"a"} trailing"#, &IN).is_err());
        assert_eq!(strict_unmarshal("", &IN).unwrap().str("file"), "");
        assert!(unmarshal("", &IN).is_err());
    }

    #[test]
    fn ignoring_errors_keeps_other_fields() {
        let o = unmarshal_ignoring_errors(r#"{"file":"a","line":"x"}"#, &IN);
        assert_eq!((o.str("file"), o.int("line")), ("a", 0));
        assert_eq!(unmarshal_ignoring_errors("{", &IN).str("file"), "");
    }

    #[test]
    fn lone_surrogates_become_replacement_chars() {
        let o = unmarshal(r#"{"file":"a\ud800b\udc00c\ud83d\ude00\\ud800"}"#, &IN).unwrap();
        assert_eq!(o.str("file"), "a\u{FFFD}b\u{FFFD}c😀\\ud800");
        assert_eq!(repair_surrogates(r#"{"a":"\n"}"#), r#"{"a":"\n"}"#);
        assert_eq!(repair_surrogates("\\ud800\\é\\"), "\\ufffd\\é\\");
    }

    #[test]
    fn compacts() {
        assert_eq!(
            compact("{ \"a\" : [1, 2], \"s\": \"x y<\" }").unwrap(),
            r#"{"a":[1,2],"s":"x y<"}"#
        );
        assert!(compact("{").is_err());
    }
}
