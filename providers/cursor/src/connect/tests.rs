//! Go `cursor/connect_test.go`, plus the error predicates of
//! `active_run_test.go` and `bridge_alive_test.go`.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::*;
use crate::testserver::serve;

#[test]
fn parse_connect_error_keeps_details() {
    let raw = br#"{"code":"internal","message":"internal error","details":[{"type":"debug","debug":{"reason":"boom"}}]}"#;
    let e = parse_connect_error(raw).unwrap();
    assert_eq!(
        (e.code.as_str(), e.message.as_str()),
        ("internal", "internal error")
    );
    assert!(e.details.contains(r#""reason":"boom""#), "{}", e.details);
    assert!(e.raw.contains(r#""details""#), "{}", e.raw);
    // The UI string stays short: details are for the log.
    assert_eq!(e.to_string(), "cursor bridge: [internal] internal error");
}

#[test]
fn parse_connect_error_rejects_empty() {
    assert_eq!(parse_connect_error(b"{}"), None);
    assert_eq!(parse_connect_error(b"not-json"), None);
}

#[test]
fn decode_end_stream_preserves_raw() {
    let err = decode_end_stream(
        br#"{"error":{"code":"internal","message":"internal error","details":[{"x":1}]}}"#,
    )
    .unwrap_err();
    assert!(err.connect().unwrap().raw.contains(r#""details""#));
    assert!(decode_end_stream(b"  ").is_ok());
    assert!(decode_end_stream(b"{}").is_ok());
    assert!(
        decode_end_stream(b"garbage").is_ok(),
        "a malformed trailer is not a failure"
    );
}

type Log = Arc<dyn Fn(&str) + Send + Sync>;

fn capture() -> (Arc<Mutex<Vec<String>>>, Log) {
    let logged = Arc::new(Mutex::new(Vec::new()));
    let l = logged.clone();
    (
        logged,
        Arc::new(move |line: &str| l.lock().push(line.to_owned())),
    )
}

#[tokio::test]
async fn stream_end_stream_logs_connect_body() {
    let srv = serve(|_req, mut resp| async move {
        resp.end(r#"{"error":{"code":"internal","message":"internal error","details":[{"hint":"from-cursor"}]}}"#)
            .await;
    })
    .await;
    let (logged, log) = capture();
    let cl = Client::new(&srv.url, "tok").with_log(log);
    let err = cl
        .stream(
            &CancellationToken::new(),
            "sdk.v1.SdkAgentService",
            "Send",
            &serde_json::json!({}),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(err.connect().unwrap().code, "internal");
    let logged = logged.lock();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(
        logged[0].contains("sdk.v1.SdkAgentService/Send"),
        "{}",
        logged[0]
    );
    assert!(logged[0].contains("from-cursor"), "{}", logged[0]);
}

#[test]
fn error_unimplemented() {
    let e = |code: &str| ConnectError {
        code: code.into(),
        ..Default::default()
    };
    assert!(e("unimplemented").unimplemented());
    assert!(!e("internal").unimplemented());
}

#[tokio::test]
async fn unary_connect_error_logs_body() {
    let srv = serve(|_req, resp| async move {
        resp.status(
            500,
            r#"{"code":"internal","message":"internal error","details":[{"hint":"unary"}]}"#,
        )
        .await;
    })
    .await;
    let (logged, log) = capture();
    let cl = Client::new(&srv.url, "tok").with_log(log);
    let err = cl
        .unary(
            &CancellationToken::new(),
            "sdk.v1.SdkAgentService",
            "CreateAgent",
            &serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(err.connect().is_some());
    let logged = logged.lock();
    assert!(
        logged.len() == 1 && logged[0].contains("unary"),
        "{logged:?}"
    );
}

#[tokio::test]
async fn non_connect_http_error_is_plain() {
    let srv = serve(|_req, resp| async move { resp.not_found().await }).await;
    let cl = Client::new(&srv.url, "tok");
    let err = cl
        .unary(
            &CancellationToken::new(),
            "svc",
            "M",
            &serde_json::json!({}),
        )
        .await
        .unwrap_err();
    assert!(err.connect().is_none());
    assert_eq!(
        err.to_string(),
        "cursor bridge: svc/M: http 404: 404 page not found\n"
    );
}

#[tokio::test]
async fn steer_run_outlives_unary_timeout() {
    let srv = serve(|req, resp| async move {
        if !req.ends("/SteerRun") {
            return resp.not_found().await;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
        resp.json(r#"{"outcome":"complete_delivered"}"#).await;
    })
    .await;
    let mut cl = Client::new(&srv.url, "tok");
    cl.unary_timeout = Duration::from_millis(40);
    let got = cl
        .steer_run(&CancellationToken::new(), "run-1", "queued behind work")
        .await
        .unwrap();
    assert_eq!(got, crate::client::STEER_COMPLETE_DELIVERED);
    // A plain unary call on the same client does time out.
    let slow = cl.ping(&CancellationToken::new()).await;
    assert!(slow.is_err());
}

#[tokio::test]
async fn unary_follows_cancel() {
    let srv = serve(|_req, resp| async move {
        resp.closed.cancelled().await;
    })
    .await;
    let cl = Client::new(&srv.url, "tok");
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        c2.cancel();
    });
    let err = cl.steer_run(&cancel, "r", "t").await.unwrap_err();
    assert!(err.is_canceled());
}

#[test]
fn active_run_conflict() {
    let conflict: Error = ConnectError {
        code: "internal".into(),
        message: "Agent agent-1 already has active run".into(),
        ..Default::default()
    }
    .into();
    assert!(conflict.connect().unwrap().active_run_conflict());
    assert!(
        conflict.clone().wrap("cursor run").active_run_conflict(),
        "wrapped *Error"
    );
    let generic: Error = ConnectError {
        code: "internal".into(),
        message: "internal error".into(),
        ..Default::default()
    }
    .into();
    assert!(!generic.active_run_conflict());
    assert!(Error::msg("x: Already Has Active Run").active_run_conflict());
}

#[test]
fn bridge_unreachable() {
    assert!(!Error::msg("cursor bridge: [invalid_argument] bad model").bridge_unreachable());
    let auth: Error = ConnectError {
        code: "unauthenticated".into(),
        message: "no key".into(),
        ..Default::default()
    }
    .into();
    assert!(
        !auth.bridge_unreachable(),
        "auth failures are not bridge-dead"
    );
    let prose = Error::msg(
        r#"cursor create agent: Post "http://127.0.0.1:61765/...": dial tcp 127.0.0.1:61765: connect: connection refused"#,
    );
    assert!(prose.bridge_unreachable());
}

#[tokio::test]
async fn refused_port_is_unreachable() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let cl = Client::new(&format!("http://127.0.0.1:{port}"), "tok");
    let err = cl
        .unary(
            &CancellationToken::new(),
            "sdk.v1.SdkAgentService",
            "CreateAgent",
            &serde_json::json!({}),
        )
        .await
        .unwrap_err()
        .wrap("cursor create agent");
    assert!(err.bridge_unreachable(), "{err}");
    assert!(
        err.to_string().starts_with(
            "cursor create agent: cursor bridge: sdk.v1.SdkAgentService/CreateAgent: "
        )
    );
}

#[test]
fn frames_round_trip() {
    let f = encode_connect_frame(true, b"{}");
    assert_eq!(f, [END_STREAM_FLAG, 0, 0, 0, 2, b'{', b'}']);
    assert_eq!(encode_connect_frame(false, b"")[0], 0);
}

#[tokio::test]
async fn stream_frames_split_across_chunks() {
    let srv = serve(|_req, mut resp| async move {
        let frame = encode_connect_frame(false, br#"{"a":1}"#);
        for b in &frame {
            resp.write(std::slice::from_ref(b)).await;
        }
        resp.write(&encode_connect_frame(false, br#"{"b":2}"#))
            .await;
        // Closed without an end frame: a clean end.
    })
    .await;
    let cl = Client::new(&srv.url, "");
    let mut got = Vec::new();
    cl.stream(
        &CancellationToken::new(),
        "s",
        "M",
        &serde_json::json!({}),
        |f| {
            got.push(String::from_utf8_lossy(f).into_owned());
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(got, [r#"{"a":1}"#, r#"{"b":2}"#]);
}

#[tokio::test]
async fn stream_truncated_body_is_an_error() {
    let srv = serve(|_req, mut resp| async move {
        let frame = encode_connect_frame(false, br#"{"a":1}"#);
        resp.write(&frame[..7]).await;
    })
    .await;
    let cl = Client::new(&srv.url, "");
    let err = cl
        .stream(
            &CancellationToken::new(),
            "s",
            "M",
            &serde_json::json!({}),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "cursor bridge: s/M: read frame body: unexpected EOF"
    );
}
