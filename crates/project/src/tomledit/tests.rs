use serde::Deserialize;

use super::{Document, Value};

#[derive(Deserialize)]
struct Op {
    op: String,
    key: Vec<String>,
    str: Option<String>,
    raw: Option<String>,
    out: String,
    err: bool,
    found: bool,
}

#[derive(Deserialize)]
struct Edit {
    name: String,
    input: String,
    ops: Vec<Op>,
}

#[derive(Deserialize)]
struct Golden {
    edit: Vec<Edit>,
}

/// Replays the edit operations in `tests/fixtures/projfile_golden.json` and
/// checks every intermediate document byte for byte.
#[test]
fn edits_match_go_toml() {
    let golden: Golden =
        serde_json::from_str(include_str!("../../tests/fixtures/projfile_golden.json")).unwrap();
    assert!(golden.edit.len() >= 20);
    for case in golden.edit {
        let mut d =
            Document::parse(&case.input).unwrap_or_else(|e| panic!("{}: parse: {e}", case.name));
        assert_eq!(d.to_string(), case.input, "{}: unmodified bytes", case.name);
        for (i, op) in case.ops.iter().enumerate() {
            let key: Vec<&str> = op.key.iter().map(String::as_str).collect();
            match op.op.as_str() {
                "set" => {
                    let v = match &op.raw {
                        Some(r) => Value::Raw(r.clone()),
                        None => Value::Str(op.str.clone().unwrap_or_default()),
                    };
                    let res = d.set(&key, &v);
                    assert_eq!(
                        res.is_err(),
                        op.err,
                        "{} op {i} set {key:?}: {res:?}",
                        case.name
                    );
                }
                "delete" => {
                    assert_eq!(
                        d.delete(&key),
                        op.found,
                        "{} op {i} delete {key:?}",
                        case.name
                    );
                }
                other => panic!("unknown op {other}"),
            }
            assert_eq!(
                d.to_string(),
                op.out,
                "{} op {i} {} {key:?}",
                case.name,
                op.op
            );
        }
    }
}

#[test]
fn get_decodes_like_unmarshal() {
    let d = Document::parse("a = 'x'\n[[run]]\nname = 'dev'\n[t]\nn = 1\n").unwrap();
    assert_eq!(d.get(&["a"]), Some(toml::Value::String("x".into())));
    assert_eq!(
        d.get(&["run", "0", "name"]),
        Some(toml::Value::String("dev".into()))
    );
    assert!(matches!(d.get(&["run"]), Some(toml::Value::Array(a)) if a.len() == 1));
    assert!(matches!(d.get(&["t"]), Some(toml::Value::Table(_))));
    assert!(matches!(d.get(&[]), Some(toml::Value::Table(_))));
    assert_eq!(d.get(&["run", "1"]), None);
    assert_eq!(d.get(&["run", "x"]), None);
    assert_eq!(d.get(&["a", "b"]), None);
    assert_eq!(d.get(&["nope"]), None);
}

#[test]
fn parse_rejects_invalid_documents() {
    assert!(Document::parse("[[[not toml").is_err());
    assert!(Document::parse("a = 1\na = 2\n").is_err());
}
