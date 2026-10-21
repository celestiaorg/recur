//! Go `feedkind_test.go`: the engine stamps `FeedItem.kind` at the source.

use std::collections::HashMap;

use recur_domain::{
    FEED_KIND_DEVIATION, FEED_KIND_NOTE, FEED_KIND_OUTCOME, FEED_KIND_TOOL, FeedItem,
};

use crate::testing::TestEngine;

#[tokio::test]
async fn feed_item_kind_stamped_at_source() {
    let te = TestEngine::new(None);
    let ws = "LOOP-FEEDKIND";
    te.engine
        .emit_workspace_event(ws, "MA", "Maestro", "Converged to 0 findings.");
    te.engine.emit_workspace_event_item(
        ws,
        FeedItem {
            agent_code: "CO".into(),
            name: "Coder".into(),
            text: "Read: <code>a.go</code>".into(),
            tool_use_id: "tu-1".into(),
            ..FeedItem::default()
        },
    );
    te.engine.emit_workspace_event_item(
        ws,
        FeedItem {
            agent_code: "CO".into(),
            name: "Coder".into(),
            text: "Looking at the code.".into(),
            kind: FEED_KIND_NOTE.into(),
            ..FeedItem::default()
        },
    );
    te.engine.emit_deviation(
        ws,
        "Departed from the default",
        "the default",
        "a good reason",
    );
    let by_text: HashMap<String, String> = te
        .store
        .feed(ws)
        .unwrap_or_default()
        .into_iter()
        .map(|it| (it.text, it.kind))
        .collect();
    for (text, kind) in [
        ("Converged to 0 findings.", FEED_KIND_OUTCOME),
        ("Read: <code>a.go</code>", FEED_KIND_TOOL),
        ("Looking at the code.", FEED_KIND_NOTE),
        ("Departed from the default", FEED_KIND_DEVIATION),
    ] {
        assert_eq!(by_text.get(text).map(String::as_str), Some(kind), "{text}");
    }
    let patched = te
        .store
        .patch_feed_by_tool_use_id(ws, "tu-1", |it| it.tool_result = "package a".into())
        .expect("tool row");
    assert_eq!(
        patched.kind, FEED_KIND_TOOL,
        "kind survives the tool_result patch"
    );
    te.close().await;
}
