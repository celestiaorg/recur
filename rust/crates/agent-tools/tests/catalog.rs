//! Catalog snapshot against Go (`golden/catalog.json`) plus the ported
//! `catalog_test.go` cases.

use std::sync::Arc;

use recur_agent_tools::coding::{
    bash::BashTool, edit::EditTool, read::ReadTool, todos::TodoTool, write::WriteTool,
};
use recur_agent_tools::{HALT_NAMES, Tool, host_tools};
use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    name: String,
    description: String,
    schema: String,
    halts: bool,
}

#[derive(Deserialize)]
struct Catalog {
    host: Vec<Entry>,
    coding: Vec<Entry>,
}

fn golden() -> Catalog {
    serde_json::from_str(include_str!("golden/catalog.json")).unwrap()
}

fn assert_same(want: &[Entry], got: &[Arc<dyn Tool>]) {
    let names: Vec<&str> = got.iter().map(|t| t.name()).collect();
    let want_names: Vec<&str> = want.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, want_names, "tool order");
    for (e, t) in want.iter().zip(got) {
        assert_eq!(t.description(), e.description, "{} description", e.name);
        assert_eq!(t.schema(), e.schema, "{} schema", e.name);
        assert_eq!(t.halts(), e.halts, "{} halts", e.name);
    }
}

#[test]
fn host_catalog_matches_go() {
    assert_same(&golden().host, &host_tools());
}

#[cfg(feature = "code")]
#[test]
fn coding_catalog_matches_go() {
    assert_same(&golden().coding, &recur_agent_tools::coding_tools());
}

#[test]
fn coding_tools_without_rg_match_go() {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadTool),
        Arc::new(EditTool),
        Arc::new(WriteTool),
        Arc::new(BashTool),
        Arc::new(TodoTool),
    ];
    let want: Vec<Entry> = golden()
        .coding
        .into_iter()
        .filter(|e| e.name != "Glob" && e.name != "Grep")
        .collect();
    assert_same(&want, &tools);
}

#[test]
fn ask_user_and_switch_mode_halt() {
    let tools = host_tools();
    for t in &tools {
        assert_eq!(t.halts(), HALT_NAMES.contains(&t.name()), "{}", t.name());
    }
    assert!(tools.iter().any(|t| t.name() == "create_workspace"));
    assert!(tools.iter().any(|t| t.name() == "message_child"));
}

#[test]
fn message_child_schema_requires_workspace_and_message() {
    let tools = host_tools();
    let tool = tools.iter().find(|t| t.name() == "message_child").unwrap();
    let schema: serde_json::Value = serde_json::from_str(&tool.schema()).unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        required.contains(&"workspace_id") && required.contains(&"message"),
        "{required:?}"
    );
}

#[test]
fn create_workspace_description_prefers_child() {
    let tools = host_tools();
    let tool = tools
        .iter()
        .find(|t| t.name() == "create_workspace")
        .unwrap();
    assert!(tool.description().contains("Prefer a child"));
}
