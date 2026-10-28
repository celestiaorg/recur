//! Go `cursor/active_run_test.go`, `client_images_test.go`, `user_test.go`
//! and the wire-shape cases of `events_test.go`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::TimeZone;
use parking_lot::Mutex;

use super::*;
use crate::connect::encode_connect_frame;
use crate::testserver::serve;

fn tok() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn first_non_terminal_run() {
    let srv = serve(|req, resp| async move {
        assert!(req.ends("/ListRuns"), "unexpected RPC {}", req.path);
        resp.json(
            r#"{"items":[
                {"runId":"run-old","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED"},
                {"runId":"run-stuck","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_RUNNING"}
            ]}"#,
        )
        .await;
    })
    .await;
    let cl = Client::new(&srv.url, "tok");
    let run = cl.first_non_terminal_run(&tok(), "agent-1").await.unwrap();
    assert_eq!(run.run_id, "run-stuck");
}

#[tokio::test]
async fn observe_run() {
    let got_run = Arc::new(Mutex::new(String::new()));
    let g = got_run.clone();
    let srv = serve(move |req, mut resp| {
        let g = g.clone();
        async move {
            assert!(req.ends("/ObserveRun"), "unexpected RPC {}", req.path);
            *g.lock() = req.str("runId");
            assert_eq!(req.json().get("afterOffset"), None, "empty offset is omitted");
            resp.frame(r#"{"result":{"agentId":"agent-1","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok"}},"offset":"1"}"#).await;
            resp.end("{}").await;
        }
    })
    .await;
    let cl = Client::new(&srv.url, "tok");
    let mut got = String::new();
    cl.observe_run(&tok(), "run-live", "", |ev| {
        if let Some(r) = ev.result.and_then(|r| r.result) {
            got = r.result;
        }
    })
    .await
    .unwrap();
    assert_eq!(*got_run.lock(), "run-live");
    assert_eq!(got, "ok");
}

#[tokio::test]
async fn cancel_non_terminal_runs() {
    let cancels = Arc::new(AtomicUsize::new(0));
    let c = cancels.clone();
    let srv = serve(move |req, resp| {
        let c = c.clone();
        async move {
            if req.ends("/ListRuns") {
                resp.json(r#"{"items":[
                    {"runId":"run-old","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED"},
                    {"runId":"run-stuck","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_RUNNING"},
                    {"runId":"run-creating","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_CREATING"}
                ]}"#)
                .await;
            } else if req.ends("/CancelRun") {
                assert_ne!(req.str("runId"), "run-old", "cancelled a terminal run");
                c.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let cl = Client::new(&srv.url, "tok");
    assert_eq!(
        cl.cancel_non_terminal_runs(&tok(), "agent-1")
            .await
            .unwrap(),
        2
    );
    assert_eq!(cancels.load(Ordering::SeqCst), 2);
}

#[test]
fn sandbox_disabled_marshals_false() {
    // The bridge only honors an explicit enabled:false.
    let b = serde_json::to_string(&LocalOptions {
        sandbox: Some(SandboxOptions { enabled: false }),
        ..Default::default()
    })
    .unwrap();
    assert!(b.contains(r#""enabled":false"#), "{b}");
}

#[test]
fn proto_time_unmarshal() {
    let snap: RunSnapshot =
        serde_json::from_str(r#"{"createdAt":"2026-08-26T20:11:42.000Z"}"#).unwrap();
    assert_eq!(
        rfc3339_nano(snap.created_at.0.unwrap()),
        "2026-08-26T20:11:42Z"
    );
    let snap: RunSnapshot =
        serde_json::from_str(r#"{"createdAt":{"seconds":"1756239102","nanos":0}}"#).unwrap();
    assert!(snap.created_at.0.is_some());
    for bad in [
        r#"{"createdAt":"garbage"}"#,
        r#"{"createdAt":null}"#,
        r#"{"createdAt":""}"#,
        r#"{"createdAt":5}"#,
        r#"{"createdAt":{"seconds":"x"}}"#,
    ] {
        let snap: RunSnapshot = serde_json::from_str(bad).unwrap();
        assert_eq!(snap.created_at.0, None, "{bad}");
    }
}

#[test]
fn proto_time_marshal() {
    assert_eq!(serde_json::to_string(&ProtoTime(None)).unwrap(), "null");
    let ts = Utc.with_ymd_and_hms(2026, 8, 26, 20, 11, 42).unwrap();
    assert_eq!(
        serde_json::to_string(&ProtoTime(Some(ts))).unwrap(),
        r#""2026-08-26T20:11:42Z""#
    );
    let frac = ts + TimeDelta::milliseconds(500);
    assert_eq!(rfc3339_nano(frac), "2026-08-26T20:11:42.5Z");
}

#[test]
fn run_created_on_this_bridge_cases() {
    let started = Utc.with_ymd_and_hms(2026, 8, 26, 20, 0, 0).unwrap();
    let run = |id: &str, at: Option<DateTime<Utc>>| RunSnapshot {
        run_id: id.into(),
        created_at: ProtoTime(at),
        ..Default::default()
    };
    assert!(run_created_on_this_bridge(
        &run("run-1", Some(started + TimeDelta::minutes(1))),
        Some(started)
    ));
    assert!(
        run_created_on_this_bridge(
            &run("run-1", Some(started - TimeDelta::milliseconds(900))),
            Some(started)
        ),
        "1s slack"
    );
    assert!(!run_created_on_this_bridge(
        &run("run-1", Some(started - TimeDelta::hours(1))),
        Some(started)
    ));
    assert!(!run_created_on_this_bridge(
        &run("run-1", None),
        Some(started)
    ));
    assert!(!run_created_on_this_bridge(
        &run("run-1", Some(started)),
        None
    ));
    assert!(!run_created_on_this_bridge(
        &run("", Some(started)),
        Some(started)
    ));
}

type BoxFut = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

fn capture_send() -> (
    Arc<Mutex<serde_json::Value>>,
    impl Fn(crate::testserver::Req, crate::testserver::Resp) -> BoxFut + Send + Sync + 'static,
) {
    let payload = Arc::new(Mutex::new(serde_json::Value::Null));
    let p = payload.clone();
    (
        payload,
        move |req: crate::testserver::Req, mut resp: crate::testserver::Resp| {
            *p.lock() = req.json();
            Box::pin(async move {
                // An empty Connect body: the client sees EOF, a clean end.
                resp.write(b"").await;
            }) as BoxFut
        },
    )
}

#[tokio::test]
async fn send_includes_images() {
    let (payload, handler) = capture_send();
    let srv = serve(handler).await;
    let cl = Client::new(&srv.url, "test-token");
    cl.send(
        &tok(),
        "agent-1",
        "see here",
        &[Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        }],
        |_| {},
    )
    .await
    .unwrap();
    let p = payload.lock();
    assert_eq!(p["message"]["text"], "see here");
    // Nested protobuf SdkImageData, not the flat TypeScript shape.
    assert_eq!(p["message"]["images"][0]["data"]["data"], "AAAA");
    assert_eq!(p["message"]["images"][0]["data"]["mimeType"], "image/png");
    assert_eq!(p["agentId"], "agent-1");
}

#[tokio::test]
async fn send_enables_deltas() {
    let (payload, handler) = capture_send();
    let srv = serve(handler).await;
    let cl = Client::new(&srv.url, "test-token");
    cl.send(&tok(), "agent-1", "hi", &[], |_| {}).await.unwrap();
    let p = payload.lock();
    assert_eq!(p["options"]["enableDeltas"], true);
    assert_eq!(p["message"].get("images"), None);
}

#[tokio::test]
async fn send_skips_unparseable_frames_and_logs_status_errors() {
    let srv = serve(|_req, mut resp| async move {
        resp.write(&encode_connect_frame(false, b"not json")).await;
        resp.frame(r#"{"sdkMessage":{"type":"status","message":{"type":"status","run_id":"r1","status":"ERROR","message":"[internal] boom"}}}"#).await;
        resp.end("{}").await;
    })
    .await;
    let logged = Arc::new(Mutex::new(Vec::<String>::new()));
    let l = logged.clone();
    let cl =
        Client::new(&srv.url, "").with_log(Arc::new(move |s: &str| l.lock().push(s.to_owned())));
    let mut n = 0;
    cl.send(&tok(), "a", "x", &[], |_| n += 1).await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        *logged.lock(),
        ["recur: run r1 status ERROR: [internal] boom"]
    );
}

#[test]
fn user_display_name_and_summary() {
    let u = User {
        api_key_name: "Recur laptop".into(),
        user_email: "dev@example.com".into(),
        user_first_name: "Josh".into(),
        user_last_name: "Dechant".into(),
        ..Default::default()
    };
    assert_eq!(u.display_name(), "Josh Dechant");
    assert_eq!(u.summary(), "dev@example.com");
    let key_only = User {
        api_key_name: "ci-bot".into(),
        ..Default::default()
    };
    assert_eq!(key_only.summary(), "ci-bot");
    let first = User {
        user_first_name: "Ada".into(),
        ..Default::default()
    };
    assert_eq!(first.display_name(), "Ada");
}

#[test]
fn mcp_server_marshals_as_oneof_wrapper() {
    // A flat {url,type} is rejected: the transport is wrapped in its oneof.
    assert_eq!(
        serde_json::to_string(&McpServer::http("http://127.0.0.1:9/tok/mcp")).unwrap(),
        r#"{"http":{"type":"HTTP_MCP_TRANSPORT_TYPE_HTTP","url":"http://127.0.0.1:9/tok/mcp"}}"#
    );
}

#[test]
fn local_options_cwd_is_repeated() {
    assert_eq!(
        serde_json::to_string(&LocalOptions {
            cwd: vec!["/tmp/wt".into()],
            ..Default::default()
        })
        .unwrap(),
        r#"{"cwd":["/tmp/wt"]}"#
    );
}

#[test]
fn version_info_has() {
    let v: VersionInfo = serde_json::from_str(r#"{"bridgeVersion":"1","protocolVersion":"sdk.v1","capabilities":["agent.create","run.observe"]}"#).unwrap();
    assert!(v.has("run.observe") && !v.has("agent.send"));
}
