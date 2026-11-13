//! Snapshot answers for argv, stdin user messages (as JSON values), model
//! flags, the background-task frame, the status card and the picker list
//! (`tests/golden/go_claude.json` and `go_claude_app.json`). Edit the fixtures
//! by hand when the behavior changes on purpose.

use std::collections::BTreeMap;

use recur_agent::{McpAttach, TurnRequest, UserImage};
use serde::Deserialize;
use serde_json::Value;

use crate::{args, flags, input, runner, status};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Image {
    id: String,
    name: String,
    mime: String,
    data: String,
    path: String,
}

impl Image {
    fn user(&self) -> UserImage {
        UserImage {
            id: self.id.clone(),
            name: self.name.clone(),
            mime: self.mime.clone(),
            data: self.data.clone(),
            path: self.path.clone(),
        }
    }
}

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
pub(crate) struct Spec {
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
    tools: Option<String>,
    permission_mode: String,
    mcp_url: String,
    playwright: bool,
    user_mcp: Option<Vec<Mcp>>,
    resume_session: String,
    fork_session: bool,
    prompt_on_stdin: bool,
    include_partials: bool,
    skip_thinking: bool,
    max_turns: u32,
    images: Option<Vec<Image>>,
    inject: bool,
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
            tools: self.tools.clone(),
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
            fork_session: self.fork_session,
            prompt_on_stdin: self.prompt_on_stdin,
            include_partials: self.include_partials,
            skip_thinking: self.skip_thinking,
            max_turns: self.max_turns,
            images: self.images.iter().flatten().map(Image::user).collect(),
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
struct ArgsRow {
    spec: Spec,
    args: Vec<String>,
    spilled: bool,
}

#[derive(Deserialize)]
struct UserRow {
    text: String,
    images: Option<Vec<Image>>,
    line: String,
}

#[derive(Deserialize)]
struct FlagRow {
    model: String,
    flags: Option<BTreeMap<String, String>>,
    resolve: String,
    window: i64,
    label: String,
}

#[derive(Deserialize)]
struct BgRow {
    line: String,
    count: usize,
    ok: bool,
}

#[derive(Deserialize)]
struct Engine {
    args: Vec<ArgsRow>,
    #[serde(rename = "userLines")]
    user_lines: Vec<UserRow>,
    flags: Vec<FlagRow>,
    background: Vec<BgRow>,
}

fn engine() -> Engine {
    serde_json::from_str(include_str!("../tests/golden/go_claude.json")).unwrap()
}

#[test]
fn argv_matches_go() {
    for row in engine().args {
        let req = row.spec.request();
        let got = args::build(&req, args::stream_input(&req, row.spec.inject));
        assert_eq!(got, row.args, "{:?}", row.spec);
        let (spilled, path) = args::spill_append_prompt(got).unwrap();
        assert_eq!(
            spilled.iter().any(|a| a == "--append-system-prompt-file"),
            row.spilled
        );
        assert_eq!(path.is_some(), row.spilled);
    }
}

#[test]
fn stdin_user_messages_match_go() {
    for row in engine().user_lines {
        let imgs: Vec<UserImage> = row.images.iter().flatten().map(Image::user).collect();
        let got = input::user_message_line(&row.text, &imgs);
        assert_eq!(got.lines().count(), 1, "{got:?}");
        assert!(got.ends_with('\n'));
        let got: serde_json::Value = serde_json::from_str(&got).unwrap();
        let want: serde_json::Value = serde_json::from_str(&row.line).unwrap();
        assert_eq!(got, want);
    }
}

#[test]
fn model_flags_match_go() {
    for row in engine().flags {
        let f = row.flags.unwrap_or_default();
        let ctx = format!("{:?} {f:?}", row.model);
        assert_eq!(flags::resolve_model(&row.model, &f), row.resolve, "{ctx}");
        assert_eq!(flags::context_window(&row.model, &f), row.window, "{ctx}");
        assert_eq!(flags::model_label(&row.model), row.label, "{ctx}");
    }
}

#[test]
fn background_frames_match_go() {
    for row in engine().background {
        let want = row.ok.then_some(row.count);
        assert_eq!(
            runner::background_task_count(row.line.as_bytes()),
            want,
            "{}",
            row.line
        );
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoStatus {
    authenticated: bool,
    path: String,
    version: String,
    provider: String,
    login_method: String,
    organization: String,
    email: String,
    error: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusRow {
    path: String,
    version: String,
    out: String,
    missing: bool,
    timed_out: bool,
    status: GoStatus,
}

#[derive(Deserialize)]
struct LoginRow {
    method: String,
    sub: String,
    out: String,
}

#[derive(Deserialize)]
struct App {
    catalog: Value,
    status: Vec<StatusRow>,
    versions: BTreeMap<String, String>,
    providers: BTreeMap<String, String>,
    logins: Vec<LoginRow>,
}

fn app() -> App {
    serde_json::from_str(include_str!("../tests/golden/go_claude_app.json")).unwrap()
}

#[test]
fn catalog_matches_go() {
    let got = serde_json::to_value(flags::catalog()).unwrap();
    assert_eq!(got, app().catalog);
}

#[test]
fn status_matches_go() {
    let app = app();
    for row in app.status {
        let st = status::classify(
            &row.path,
            &row.version,
            row.out.as_bytes(),
            row.missing,
            row.timed_out,
        );
        let g = &row.status;
        assert_eq!(
            (
                st.authenticated,
                st.path.as_str(),
                st.version.as_str(),
                st.provider.as_str(),
                st.login_method.as_str(),
                st.organization.as_str(),
                st.email.as_str(),
                st.error.as_str()
            ),
            (
                g.authenticated,
                g.path.as_str(),
                g.version.as_str(),
                g.provider.as_str(),
                g.login_method.as_str(),
                g.organization.as_str(),
                g.email.as_str(),
                g.error.as_str()
            ),
            "{} missing={} timed_out={}",
            row.out,
            row.missing,
            row.timed_out
        );
    }
    for (raw, want) in app.versions {
        assert_eq!(status::parse_version(&raw), want, "{raw:?}");
    }
    for (raw, want) in app.providers {
        assert_eq!(status::format_provider(&raw), want, "{raw:?}");
    }
    for row in app.logins {
        assert_eq!(
            status::format_login_method(&row.method, &row.sub),
            row.out,
            "{:?} {:?}",
            row.method,
            row.sub
        );
    }
}
