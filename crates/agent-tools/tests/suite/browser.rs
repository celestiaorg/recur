//! browser_screenshot: input bounds, the capped result and the host call.

use std::sync::{Arc, Mutex};

use recur_agent_tools::host_tools::browser_shots_result;
use recur_agent_tools::{
    BROWSER_UNAVAILABLE, BrowserShot, BrowserShots, Call, Host, HostError, NoopHost, host_tools,
    names,
};
use serde_json::Value;

#[test]
fn widths_default_dedupe_and_bound() {
    use recur_agent_tools::host_tools::parse_widths;
    assert_eq!(parse_widths(&[]).unwrap(), [1440, 1024, 768, 390]);
    assert_eq!(parse_widths(&[800, 390, 800]).unwrap(), [800, 390]);
    assert_eq!(parse_widths(&[320, 2560]).unwrap(), [320, 2560]);
    assert!(parse_widths(&[319]).unwrap_err().contains("out of range"));
    assert!(parse_widths(&[2561]).is_err());
    assert!(parse_widths(&[-1]).is_err());
    assert!(parse_widths(&[400; 7]).unwrap_err().contains("at most 6"));
}

#[test]
fn result_caps_and_marks_page_text_untrusted() {
    let shots = BrowserShots {
        url: "http://localhost:3000".into(),
        shots: vec![
            BrowserShot {
                width: 1440,
                height: 2000,
                path: "/d/1-1440.png".into(),
                error: String::new(),
            },
            BrowserShot {
                width: 390,
                error: "timed out after 25s".into(),
                ..BrowserShot::default()
            },
        ],
        console_errors: (0..30)
            .map(|i| format!("err {i}"))
            .chain(["err 0".into(), "y".repeat(800)])
            .collect(),
        exceptions: vec!["boom".into(), "  ".into()],
        failed_requests: (0..20).map(|i| format!("fetch /api/{i} 500")).collect(),
    };
    let v: Value = serde_json::from_str(&browser_shots_result(&shots)).unwrap();
    assert_eq!(v["url"], "http://localhost:3000");
    assert_eq!(
        v["screenshots"],
        serde_json::json!([
            {"width": 1440, "path": "/d/1-1440.png", "height": 2000},
            {"width": 390, "error": "timed out after 25s"},
        ])
    );
    let out = &v["page_output"];
    assert!(out["note"].as_str().unwrap().contains("untrusted"));
    let console = out["console_errors"].as_array().unwrap();
    assert_eq!(console.len(), 31, "deduplicated");
    assert_eq!(console[30].as_str().unwrap().chars().count(), 501);
    assert_eq!(out["exceptions"], serde_json::json!(["boom"]));
    assert_eq!(out["failed_requests"].as_array().unwrap().len(), 40 - 32);
    assert_eq!(out["omitted"], 12);
}

#[derive(Default)]
struct Shots(Mutex<Vec<(String, String, Vec<u32>)>>);

#[async_trait::async_trait]
impl Host for Shots {
    async fn browser_screenshot(
        &self,
        ws: &str,
        _agent: &str,
        url: &str,
        widths: &[u32],
    ) -> Result<BrowserShots, HostError> {
        self.0
            .lock()
            .unwrap()
            .push((ws.into(), url.into(), widths.to_vec()));
        Ok(BrowserShots {
            url: "http://localhost:3000".into(),
            ..BrowserShots::default()
        })
    }
}

async fn run(host: Arc<dyn Host>, args: &str) -> Result<String, String> {
    let tools = host_tools();
    let tool = tools
        .iter()
        .find(|t| t.name() == names::BROWSER_SCREENSHOT)
        .expect("registered");
    tool.execute(Call {
        workspace_id: "ws1".into(),
        args: args.into(),
        host: Some(host),
        ..Default::default()
    })
    .await
    .map_err(|e| e.0)
}

#[tokio::test]
async fn tool_parses_args_and_calls_the_host() {
    let host = Arc::new(Shots::default());
    run(host.clone(), "").await.unwrap();
    run(
        host.clone(),
        r#"{"url":" http://x.test/a ","widths":[800]}"#,
    )
    .await
    .unwrap();
    assert!(run(host.clone(), r#"{"widths":[100]}"#).await.is_err());
    assert!(run(host.clone(), r#"{"widths":"wide"}"#).await.is_err());
    assert_eq!(
        *host.0.lock().unwrap(),
        [
            ("ws1".into(), String::new(), vec![1440, 1024, 768, 390]),
            ("ws1".into(), "http://x.test/a".into(), vec![800]),
        ]
    );
    assert_eq!(
        run(Arc::new(NoopHost), "{}").await.unwrap_err(),
        BROWSER_UNAVAILABLE
    );
}

#[test]
fn schema_lists_url_and_widths() {
    let tools = host_tools();
    let tool = tools
        .iter()
        .find(|t| t.name() == names::BROWSER_SCREENSHOT)
        .unwrap();
    let schema: Value = serde_json::from_str(&tool.schema()).unwrap();
    assert_eq!(schema["properties"]["widths"]["items"]["type"], "integer");
    assert_eq!(schema["properties"]["url"]["type"], "string");
    assert!(schema.get("required").is_none());
    assert!(tool.description().contains("untrusted"));
}
