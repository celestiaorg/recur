//! schedule_wakeup: the notice turn fires once when due (also when overdue
//! on launch), a new call replaces the pending one, and closing the chat
//! cancels it.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone, Utc};
use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent_tools::Host;

use super::test_support::*;
use crate::host::EngineHost;
use crate::testing::wait_for;

const T0: i64 = 1_790_000_000_000;

fn at(ms: i64) -> chrono::DateTime<Local> {
    Utc.timestamp_millis_opt(ms).unwrap().with_timezone(&Local)
}

#[tokio::test(flavor = "multi_thread")]
async fn wakeup_fires_once_and_a_new_call_replaces_it() {
    let runner = capture_same("wake", "checked");
    let fx = Fx::new(Some(runner.provider()));
    let ws = fx.workspace("wake").await;
    fx.eng().ensure_pair_agent(&ws.id, "PA", "");
    fx.eng().set_clock(|| at(T0));
    let h = EngineHost::new(Arc::downgrade(fx.eng()));
    let ok = h.schedule_wakeup(&ws.id, "PA", 600, "first").await.unwrap();
    assert!(ok.contains("in 10 min"), "{ok}");
    let due = crate::chat::wakeups::wakeup_due(&ok).expect("due stamp");
    let left = due.timestamp() - fx.eng().now().timestamp();
    assert!((599..=601).contains(&left), "{ok}");
    assert!(crate::chat::wakeups::wakeup_due("Wakeup set").is_none());
    h.schedule_wakeup(&ws.id, "PA", 60, "CI run").await.unwrap();
    fx.eng()
        .tick_scheduled_prompts(at(T0 + 59_000), false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runner.specs.lock().is_empty(), "not due yet");

    fx.eng()
        .tick_scheduled_prompts(at(T0 + 3_600_000), true)
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "wakeup turn", || {
        !runner.specs.lock().is_empty()
    })
    .await;
    fx.eng()
        .tick_scheduled_prompts(at(T0 + 7_200_000), false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let turns = runner.specs.lock().clone();
    assert_eq!(
        turns.len(),
        1,
        "fires once; the replaced wakeup never fires"
    );
    assert!(
        turns[0].prompt.contains("Reason: CI run"),
        "{}",
        turns[0].prompt
    );
    let label = format!("{DISPLAY_HIDDEN_SENTINEL}Wakeup: CI run");
    let items = fx.items(&ws.id, "PA").await;
    assert!(
        items.iter().any(|i| i.role == "user" && i.text == label),
        "{items:?}"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_chat_cancels_its_wakeup() {
    let runner = capture_same("wake", "checked");
    let fx = Fx::new(Some(runner.provider()));
    let ws = fx.workspace("wake").await;
    fx.eng().ensure_pair_agent(&ws.id, "PA", "");
    fx.eng().ensure_pair_agent(&ws.id, "PA2", "");
    fx.eng().set_clock(|| at(T0));
    let h = EngineHost::new(Arc::downgrade(fx.eng()));
    h.schedule_wakeup(&ws.id, "PA2", 60, "gone").await.unwrap();
    fx.eng().close_workspace_chat(&ws.id, "PA2").await;
    fx.eng()
        .tick_scheduled_prompts(at(T0 + 120_000), false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(runner.specs.lock().is_empty());
    assert!(
        h.schedule_wakeup("nope", "PA", 60, "x").await.is_err(),
        "unknown workspace"
    );
    fx.te.close().await;
}
