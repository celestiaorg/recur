//! Go-recorded tool outputs compare loosely: JSON as values, and a Go
//! `encoding/json` error matches any error with the same Recur prefix.

use serde_json::Value;

pub fn same_result(got: &str, want: &str) -> bool {
    let json = |s: &str| {
        serde_json::from_str::<Value>(s)
            .ok()
            .map(recur_domain::json::canonical)
    };
    got == want || json(got).is_some_and(|g| Some(g) == json(want))
}

const GO_DECODE: &[&str] = &[
    "json: ",
    "unexpected end of JSON input",
    "unexpected EOF",
    "invalid character ",
    "EOF",
];

pub fn same_error(got: &str, want: &str) -> bool {
    let Some(at) = GO_DECODE.iter().filter_map(|m| want.find(m)).min() else {
        return got == want;
    };
    got.len() > at && got.starts_with(&want[..at])
}
