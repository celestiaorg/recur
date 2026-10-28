//! Replays `tests/golden/go_usage.json`, recorded from the Go usage package
//! (generator next to it as `gen_go_usage.go.txt`).

use serde_json::{Value, json};

use crate::claude::{
    parse_claude_account_file, parse_claude_oauth, parse_claude_usage, scoped_window_id,
    scoped_window_label,
};
use crate::codex::{
    codex_monthly_usage_url, format_codex_credit_qty, parse_codex_auth, parse_codex_monthly_usage,
    parse_codex_usage,
};
use crate::cursor::oauth::parse_cursor_poll;
use crate::cursor::session::{CursorTok, write_cursor_auth_file};
use crate::cursor::{parse_cursor_models, parse_cursor_period};
use crate::parse::{email_from_cursor_json, email_from_jwt, jwt_claim, parse_resets_at};
use crate::*;

fn golden() -> Value {
    serde_json::from_str(include_str!("../../tests/golden/go_usage.json")).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// Go marshals nil slices as `null`; compare through the Rust encoding.
fn enc<T: serde::Serialize>(v: &T) -> Value {
    recur_domain::json::canonical(serde_json::to_value(v).unwrap())
}

fn quota_cases(key: &str, parse: fn(&[u8]) -> Result<ProviderQuota, String>) {
    for c in golden()[key].as_array().unwrap() {
        let raw = s(&c["raw"]);
        match parse(raw.as_bytes()) {
            Ok(q) => {
                assert!(!c["err"].as_bool().unwrap(), "{raw}: want error, got {q:?}");
                assert_eq!(enc(&q), c["quota"], "{raw}");
            }
            Err(e) => assert!(c["err"].as_bool().unwrap(), "{raw}: {e}"),
        }
    }
}

#[test]
fn claude_usage_matches_go() {
    quota_cases("claudeUsage", parse_claude_usage);
}

#[test]
fn codex_usage_matches_go() {
    quota_cases("codexUsage", parse_codex_usage);
}

#[test]
fn codex_monthly_matches_go() {
    for c in golden()["codexMonthly"].as_array().unwrap() {
        assert_eq!(
            enc(&parse_codex_monthly_usage(s(&c["raw"]).as_bytes())),
            c["window"],
            "{}",
            c["raw"]
        );
    }
}

#[test]
fn codex_auth_matches_go() {
    for c in golden()["codexAuth"].as_array().unwrap() {
        let raw = s(&c["raw"]);
        match parse_codex_auth(raw.as_bytes()) {
            Ok(a) => {
                assert!(!c["err"].as_bool().unwrap(), "{raw}");
                assert_eq!(
                    json!({"accessToken": a.access_token, "idToken": a.id_token, "accountId": a.account_id,
                        "email": a.email, "hasApiKey": a.has_api_key, "hasChatGpt": a.has_chatgpt}),
                    c["auth"],
                    "{raw}"
                );
            }
            Err(e) => assert!(c["err"].as_bool().unwrap(), "{raw}: {e}"),
        }
    }
}

#[test]
fn cursor_period_matches_go() {
    for c in golden()["cursorPeriod"].as_array().unwrap() {
        let raw = s(&c["raw"]);
        match parse_cursor_period(raw.as_bytes()) {
            Ok(p) => {
                assert!(!c["err"].as_bool().unwrap(), "{raw}");
                let want = &c["period"];
                assert_eq!(enc(&p.quota), want["quota"], "{raw}");
                assert_eq!(
                    (p.start.as_str(), p.end.as_str()),
                    (s(&want["start"]), s(&want["end"]))
                );
                assert_eq!(
                    (p.auto_pct, p.api_pct, p.total_pct),
                    (
                        want["autoPct"].as_f64().unwrap(),
                        want["apiPct"].as_f64().unwrap(),
                        want["totalPct"].as_f64().unwrap()
                    )
                );
            }
            Err(e) => assert!(c["err"].as_bool().unwrap(), "{raw}: {e}"),
        }
    }
}

#[test]
fn cursor_models_match_go() {
    for c in golden()["cursorModels"].as_array().unwrap() {
        let rows = parse_cursor_models(
            s(&c["raw"]).as_bytes(),
            c["auto"].as_f64().unwrap(),
            c["api"].as_f64().unwrap(),
            c["total"].as_f64().unwrap(),
        );
        let want = if c["rows"].is_null() {
            json!([])
        } else {
            c["rows"].clone()
        };
        assert_eq!(enc(&rows), want, "{}", c["raw"]);
    }
}

#[test]
fn resets_at_matches_go() {
    for c in golden()["resetsAt"].as_array().unwrap() {
        let v: Value = serde_json::from_str(s(&c["raw"])).unwrap();
        let v = (!v.is_null()).then_some(v);
        assert_eq!(
            parse_resets_at(v.as_ref()),
            c["out"].as_i64().unwrap(),
            "{}",
            c["raw"]
        );
    }
}

#[test]
fn scoped_windows_match_go() {
    for c in golden()["scoped"].as_array().unwrap() {
        let name = s(&c["name"]);
        assert_eq!(scoped_window_id(name), s(&c["id"]), "{name:?}");
        assert_eq!(scoped_window_label(name), s(&c["label"]), "{name:?}");
    }
}

#[test]
fn credit_qty_matches_go() {
    for c in golden()["creditQty"].as_array().unwrap() {
        assert_eq!(
            format_codex_credit_qty(c["n"].as_f64().unwrap()),
            s(&c["out"]),
            "{}",
            c["n"]
        );
    }
}

#[test]
fn monthly_url_matches_go() {
    for c in golden()["monthlyURL"].as_array().unwrap() {
        assert_eq!(
            codex_monthly_usage_url(s(&c["usage"]), s(&c["account"])),
            s(&c["out"]),
            "{c}"
        );
    }
}

#[test]
fn jwt_emails_match_go() {
    for c in golden()["jwtEmails"].as_array().unwrap() {
        let tok = s(&c["tok"]);
        assert_eq!(email_from_jwt(tok), s(&c["email"]), "{tok}");
        assert_eq!(jwt_claim(tok, "sub"), s(&c["sub"]), "{tok}");
    }
    for c in golden()["cursorEmails"].as_array().unwrap() {
        let m: Value = serde_json::from_str(s(&c["raw"])).unwrap();
        assert_eq!(
            email_from_cursor_json(m.as_object()),
            s(&c["email"]),
            "{}",
            c["raw"]
        );
    }
}

#[test]
fn claude_oauth_matches_go() {
    for c in golden()["claudeOAuth"].as_array().unwrap() {
        let got = parse_claude_oauth(s(&c["raw"]).as_bytes());
        assert_eq!(
            json!({"access": got.access, "refresh": got.refresh, "expiresAt": got.expires_at}),
            json!({"access": c["access"], "refresh": c["refresh"], "expiresAt": c["expiresAt"]}),
            "{}",
            c["raw"]
        );
    }
    for c in golden()["claudeAccounts"].as_array().unwrap() {
        assert_eq!(
            parse_claude_account_file(s(&c["raw"]).as_bytes()),
            s(&c["email"])
        );
    }
}

#[test]
fn cursor_auth_files_match_go() {
    let want = golden()["cursorAuthFiles"].clone();
    let dir = tempfile::tempdir().unwrap();
    let toks = [
        CursorTok {
            access: "sess<1>".into(),
            refresh: "r".into(),
            email: "e@x.io".into(),
            api_key: "crsr_k".into(),
            api_key_expires_at_ms: 1_788_220_800_000,
            api_key_id: 22,
            ..Default::default()
        },
        CursorTok {
            access: "s".into(),
            ..Default::default()
        },
    ];
    for (i, tok) in toks.iter().enumerate() {
        let p = dir.path().join(format!("{i}.json"));
        write_cursor_auth_file(&p, tok).unwrap();
        let got: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(got, serde_json::from_str::<Value>(s(&want[i])).unwrap());
    }
}

#[test]
fn cursor_polls_match_go() {
    for c in golden()["cursorPolls"].as_array().unwrap() {
        let raw = s(&c["raw"]);
        match parse_cursor_poll(raw.as_bytes()) {
            Ok(t) => {
                assert!(!c["err"].as_bool().unwrap(), "{raw}");
                assert_eq!(
                    (t.access.as_str(), t.refresh.as_str(), t.email.as_str()),
                    (s(&c["access"]), s(&c["refresh"]), s(&c["email"]))
                );
            }
            Err(e) => assert!(c["err"].as_bool().unwrap(), "{raw}: {e}"),
        }
    }
}

#[test]
fn merge_stale_matches_go() {
    for c in golden()["mergeStale"].as_array().unwrap() {
        let prev: Snapshot = serde_json::from_value(c["prev"].clone()).unwrap();
        let next: Snapshot = serde_json::from_value(c["next"].clone()).unwrap();
        assert_eq!(enc(&merge_stale(&prev, next)), c["out"], "{c}");
    }
}

#[test]
fn local_rows_match_go() {
    let g = golden();
    let st = &g["stubs"];
    assert_eq!(enc(&cursor_no_session_quota()), st["noSession"]);
    assert_eq!(enc(&cursor_connect_tokens_quota()), st["tokens"]);
    assert_eq!(enc(&cursor_connecting_quota()), st["connecting"]);
    assert_eq!(enc(&empty_snapshot()), st["empty"]);
}
