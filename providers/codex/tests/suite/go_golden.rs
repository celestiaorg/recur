//! Snapshot answers for Codex: `-c` overrides, user prompts, sandbox and
//! approval policy, notification → event mapping, the usage meter, approval
//! answers, file-change enrichment, the display boundary, request encodings,
//! and the picker list (`golden/go_codex.json`, `golden/go_codex_app.json`).
//! Edit the fixture by hand when the behavior changes on purpose.

use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use recur_agent::{AgentEvent, McpAttach, Provider, TurnRequest};
use recur_domain::PermissionDecision;
use recur_provider_codex::approvals::{
    FileItems, agent_code, approval_policy, approval_response, approval_tool, enrich_params,
};
use recur_provider_codex::items::{
    TurnState, item_completed, item_started, observe_usage, turn_usage,
};
use recur_provider_codex::mcp::{McpServer, config_overrides};
use recur_provider_codex::rpc::strip_nulls;
use recur_provider_codex::runner::{
    long_context_overrides, mcp_servers_for, network_overrides, parse_context_flag, sandbox_for,
    service_tier_from_flags, user_prompt,
};
use recur_provider_codex::server::auto_approve;
use recur_provider_codex::types::{
    Model, SandboxPolicy, ThreadStartParams, TurnInterruptParams, TurnStartParams, TurnSteerParams,
    UserInput, format_plan,
};
use recur_provider_codex::{CodexConfig, CodexProvider, catalog, status};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Mcp {
    name: String,
    url: String,
    headers: Option<BTreeMap<String, String>>,
    command: String,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

/// The fixture's projection of an AgentSpec.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Spec {
    workspace_id: String,
    agent_id: String,
    role: String,
    prompt: String,
    model: String,
    flags: Option<BTreeMap<String, String>>,
    system_prompt: String,
    append_system_prompt: String,
    turn_append_prompt: String,
    refresh_standing: bool,
    allowed_tools: Option<Vec<String>>,
    permission_mode: String,
    mcp_url: String,
    playwright: bool,
    user_mcp: Option<Vec<Mcp>>,
    resume_session: String,
}

impl Spec {
    fn request(&self) -> TurnRequest {
        TurnRequest {
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            role: self.role.as_str().into(),
            prompt: self.prompt.clone(),
            model: self.model.clone(),
            flags: self.flags.clone().unwrap_or_default(),
            system_prompt: self.system_prompt.clone(),
            append_system_prompt: self.append_system_prompt.clone(),
            turn_append_prompt: self.turn_append_prompt.clone(),
            refresh_standing: self.refresh_standing,
            allowed_tools: self.allowed_tools.clone(),
            permission_mode: self.permission_mode.clone(),
            mcp_url: self.mcp_url.clone(),
            playwright: self.playwright,
            user_mcp: self
                .user_mcp
                .iter()
                .flatten()
                .map(|u| McpAttach {
                    name: u.name.clone(),
                    url: u.url.clone(),
                    headers: u.headers.clone().unwrap_or_default(),
                    command: u.command.clone(),
                    args: u.args.clone().unwrap_or_default(),
                    env: u.env.clone().unwrap_or_default(),
                })
                .collect(),
            resume_session: self.resume_session.clone(),
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
struct Golden {
    overrides: Vec<OverridesRow>,
    specs: Vec<SpecRow>,
    items: Vec<ItemRow>,
    usage: Vec<UsageRow>,
    approvals: Vec<ApprovalRow>,
    enrich: Vec<EnrichRow>,
    display: Vec<DisplayRow>,
    params: Vec<ParamRow>,
    #[serde(rename = "contextFlags")]
    context_flags: Vec<ContextRow>,
    #[serde(rename = "formatPlan")]
    format_plan: BTreeMap<String, String>,
    #[serde(rename = "networkOn")]
    network_on: Vec<String>,
    #[serde(rename = "approvalModes")]
    approval_modes: BTreeMap<String, String>,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("../golden/go_codex.json")).unwrap()
}

/// An MCP server as the fixture encodes it, with its field names.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GoServer {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "Required")]
    required: bool,
    #[serde(rename = "Command")]
    command: String,
    #[serde(rename = "Args")]
    args: Option<Vec<String>>,
    #[serde(rename = "Env")]
    env: Option<BTreeMap<String, String>>,
    #[serde(rename = "HTTPHeaders")]
    headers: Option<BTreeMap<String, String>>,
}

impl GoServer {
    fn server(self) -> McpServer {
        McpServer {
            name: self.name,
            url: self.url,
            required: self.required,
            command: self.command,
            args: self.args.unwrap_or_default(),
            env: self.env.unwrap_or_default(),
            http_headers: self.headers.unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct OverridesRow {
    servers: Option<Vec<GoServer>>,
    overrides: Vec<String>,
}

#[test]
fn overrides_match_go() {
    for row in golden().overrides {
        let servers: Vec<McpServer> = row
            .servers
            .into_iter()
            .flatten()
            .map(GoServer::server)
            .collect();
        assert_eq!(config_overrides(&servers), row.overrides);
    }
    assert_eq!(network_overrides(true), golden().network_on);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecRow {
    spec: Spec,
    prompt_fresh: String,
    prompt_resumed: String,
    sandbox: String,
    policy: String,
    sandbox_net: String,
    policy_net: String,
    approval: String,
    service_tier: String,
    long_context: Option<Vec<String>>,
    servers: Option<Vec<String>>,
    agent_code: String,
}

#[test]
fn turn_shaping_matches_go() {
    for row in golden().specs {
        let req = row.spec.request();
        let ctx = format!("{:?}", row.spec);
        assert_eq!(user_prompt(&req, false), row.prompt_fresh, "{ctx}");
        assert_eq!(user_prompt(&req, true), row.prompt_resumed, "{ctx}");
        let enc = |p: &SandboxPolicy| serde_json::to_string(p).unwrap();
        let (s, p) = sandbox_for(&req, false);
        assert_eq!(
            (s, enc(&p)),
            (row.sandbox.as_str(), row.policy.clone()),
            "{ctx}"
        );
        let (s, p) = sandbox_for(&req, true);
        assert_eq!(
            (s, enc(&p)),
            (row.sandbox_net.as_str(), row.policy_net.clone()),
            "{ctx}"
        );
        assert_eq!(approval_policy(&req.permission_mode), row.approval, "{ctx}");
        assert_eq!(
            service_tier_from_flags(&req.flags),
            row.service_tier,
            "{ctx}"
        );
        assert_eq!(
            long_context_overrides(&req.flags),
            row.long_context.unwrap_or_default(),
            "{ctx}"
        );
        let want: Vec<McpServer> = row
            .servers
            .unwrap_or_default()
            .iter()
            .map(|s| serde_json::from_str::<GoServer>(s).unwrap().server())
            .collect();
        assert_eq!(mcp_servers_for(&req), want, "{ctx}");
        assert_eq!(
            agent_code(&req.workspace_id, &req.agent_id),
            row.agent_code,
            "{ctx}"
        );
    }
    for (mode, want) in golden().approval_modes {
        assert_eq!(approval_policy(&mode), want, "{mode:?}");
    }
}

#[derive(Deserialize)]
struct ItemRow {
    method: String,
    params: String,
    events: Vec<BTreeMap<String, Value>>,
}

fn normalize(ev: &AgentEvent) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::from([("kind".to_owned(), json!(ev.kind()))]);
    let mut set = |k: &str, v: &str| {
        if !v.is_empty() {
            m.insert(k.into(), json!(v));
        }
    };
    match ev {
        AgentEvent::Step(s) => {
            set("tool", &s.name);
            set("toolInput", &s.summary);
            set("toolInputRaw", &s.input);
            set("toolUseId", &s.id);
        }
        AgentEvent::ToolResult(r) => {
            set("tool", &r.name);
            set("toolInputRaw", &r.input);
            set("toolUseId", &r.id);
            set("text", &r.text);
            if r.is_error {
                m.insert("isError".into(), json!(true));
            }
        }
        AgentEvent::Text { text, .. } | AgentEvent::Thinking { text, .. } => set("text", text),
        other => panic!("unexpected {other:?}"),
    }
    m
}

/// A JSON-text field as its value, so encodings of equal input compare equal.
fn json_text(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| json!(s))
}

fn decode_raw(mut m: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    if let Some(Value::String(raw)) = m.get("toolInputRaw") {
        let v = json_text(raw);
        m.insert("toolInputRaw".into(), v);
    }
    m
}

/// Runs every notification in order through shared turn state, as the
/// handler does, and returns the file items for the enrichment check.
fn replay_items() -> FileItems {
    let items = FileItems::default();
    let mut st = TurnState::default();
    for row in golden().items {
        let ev = if row.method == "item/started" {
            item_started(&row.params, &mut st, &items)
        } else {
            item_completed(&row.params, &mut st)
        };
        let got: Vec<_> = ev.iter().map(|e| decode_raw(normalize(e))).collect();
        let want: Vec<_> = row.events.into_iter().map(decode_raw).collect();
        assert_eq!(got, want, "{} {}", row.method, row.params);
    }
    items
}

#[test]
fn notifications_match_go() {
    replay_items();
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnrichRow {
    method: String,
    params: String,
    out: String,
}

#[test]
fn approval_enrichment_matches_go() {
    let items = replay_items();
    for row in golden().enrich {
        assert_eq!(
            json_text(&enrich_params(&row.method, &row.params, &items)),
            json_text(&row.out),
            "{}",
            row.params
        );
    }
}

#[derive(Deserialize)]
struct Meter {
    #[serde(rename = "in")]
    input: i64,
    out: i64,
    cr: i64,
    cc: i64,
    cw: i64,
}

#[derive(Deserialize)]
struct UsageRow {
    params: String,
    last: BTreeMap<String, i64>,
    event: Meter,
    turn: Meter,
}

#[test]
fn usage_meter_matches_go() {
    let mut st = TurnState::default();
    for row in golden().usage {
        let snap = observe_usage(&row.params);
        let l = snap.last;
        assert_eq!(
            [
                l.total_tokens,
                l.input_tokens,
                l.cached_input_tokens,
                l.cache_write_input_tokens,
                l.output_tokens,
                l.reasoning_output_tokens
            ],
            [
                "totalTokens",
                "inputTokens",
                "cachedInputTokens",
                "cacheWriteInputTokens",
                "outputTokens",
                "reasoningOutputTokens"
            ]
            .map(|k| row.last[k]),
            "{}",
            row.params
        );
        let AgentEvent::Usage(u) = st.observe_usage(&row.params) else {
            panic!()
        };
        let t = turn_usage(&st.usage_last, &st.usage_turn, st.context_window);
        for (got, want) in [(u, &row.event), (t, &row.turn)] {
            assert_eq!(
                [
                    got.input_tokens,
                    got.output_tokens,
                    got.cache_read_tokens,
                    got.cache_creation_tokens,
                    got.context_window
                ],
                [want.input, want.out, want.cr, want.cc, want.cw],
                "{}",
                row.params
            );
        }
    }
}

#[derive(Deserialize)]
struct ApprovalRow {
    method: String,
    params: String,
    allow: bool,
    remember: bool,
    tool: String,
    human: bool,
    response: String,
    auto: String,
}

#[test]
fn approval_answers_match_go() {
    for row in golden().approvals {
        assert_eq!(
            approval_tool(&row.method),
            row.human.then_some(row.tool.as_str())
        );
        let d = PermissionDecision {
            allow: row.allow,
            remember: row.remember,
            ..Default::default()
        };
        let got = serde_json::to_string(&approval_response(&row.method, &d, &row.params)).unwrap();
        assert_eq!(got, row.response, "{} {} {d:?}", row.method, row.params);
        let auto = serde_json::to_string(&auto_approve(&row.method, "").unwrap()).unwrap();
        assert_eq!(auto, row.auto, "{}", row.method);
    }
}

#[derive(Deserialize)]
struct DisplayRow {
    name: String,
    input: String,
    out: String,
    tool: String,
    summary: String,
}

#[test]
fn display_matches_go() {
    let p = CodexProvider::new(CodexConfig::default());
    for row in golden().display {
        let d = p.display_tool(&row.name, &row.input);
        assert_eq!(
            (d.name.as_str(), json_text(&d.input)),
            (row.tool.as_str(), json_text(&row.out)),
            "{} {}",
            row.name,
            row.input
        );
        assert_eq!(
            recur_agent::display::summarize_display_input(&p, &row.name, &row.input),
            row.summary,
            "{} {}",
            row.name,
            row.input
        );
    }
}

#[derive(Deserialize)]
struct ParamRow {
    name: String,
    json: String,
}

#[test]
fn request_encodings_match_go() {
    let enc = |v: &dyn erased::Enc| v.enc();
    let text = |t: &str| UserInput::text(t);
    for row in golden().params {
        let got = match row.name.as_str() {
            "turnStart" => enc(&TurnStartParams {
                thread_id: "thr".into(),
                input: vec![text("fix <it> & go")],
                model: "gpt-5.6-sol".into(),
                cwd: "/w".into(),
                approval_policy: "on-request".into(),
                sandbox_policy: Some(SandboxPolicy {
                    typ: "readOnly".into(),
                    network_access: true,
                }),
                effort: "high".into(),
                service_tier: "priority".into(),
                collaboration: None,
            }),
            "turnStartMin" => enc(&TurnStartParams {
                thread_id: "thr".into(),
                input: vec![UserInput {
                    typ: "text".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            "threadStart" => enc(&ThreadStartParams {
                model: "m".into(),
                cwd: "/w".into(),
                approval_policy: "never".into(),
                sandbox: "danger-full-access".into(),
                developer_instructions: "You are <the> Coder.".into(),
                service_name: "recur".into(),
                ..Default::default()
            }),
            "threadStartEmpty" => enc(&ThreadStartParams::default()),
            "turnSteer" => enc(&TurnSteerParams {
                thread_id: "thr".into(),
                input: vec![text("steer")],
            }),
            "turnInterrupt" => enc(&TurnInterruptParams {
                thread_id: "thr".into(),
                turn_id: "turn".into(),
            }),
            other => panic!("unknown param row {other}"),
        };
        assert_eq!(json_text(&got), json_text(&row.json), "{}", row.name);
    }
}

mod erased {
    pub trait Enc {
        fn enc(&self) -> String;
    }
    impl<T: serde::Serialize> Enc for T {
        fn enc(&self) -> String {
            serde_json::to_string(self).unwrap()
        }
    }
}

#[derive(Deserialize)]
struct ContextRow {
    #[serde(rename = "in")]
    input: String,
    out: i64,
}

#[test]
fn small_tables_match_go() {
    let g = golden();
    for row in g.context_flags {
        assert_eq!(parse_context_flag(&row.input), row.out, "{:?}", row.input);
    }
    for (raw, want) in g.format_plan {
        assert_eq!(format_plan(&raw), want, "{raw:?}");
    }
}

#[derive(Deserialize)]
struct App {
    models: Value,
    catalog: Value,
    labels: Vec<LabelRow>,
    efforts: BTreeMap<String, String>,
    logins: Vec<LoginRow>,
}

#[derive(Deserialize)]
struct LabelRow {
    display: String,
    id: String,
    out: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginRow {
    #[serde(rename = "type")]
    typ: String,
    has_key: bool,
    out: String,
}

#[test]
fn catalog_matches_go() {
    let app: App = serde_json::from_str(include_str!("../golden/go_codex_app.json")).unwrap();
    let mut models = app.models;
    strip_nulls(&mut models);
    let models: Vec<Model> = serde_json::from_value(models).unwrap();
    assert_eq!(
        serde_json::to_value(catalog::catalog_models(&models)).unwrap(),
        app.catalog
    );
    for row in app.labels {
        assert_eq!(
            catalog::friendly_model_label(&row.display, &row.id),
            row.out,
            "{:?} {:?}",
            row.display,
            row.id
        );
    }
    for (raw, want) in app.efforts {
        assert_eq!(catalog::friendly_effort_label(&raw), want, "{raw:?}");
    }
    for row in app.logins {
        assert_eq!(
            status::login_method(&row.typ, row.has_key),
            row.out,
            "{:?} {}",
            row.typ,
            row.has_key
        );
    }
}
