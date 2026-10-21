//! Typed `sdk.v1` calls (Go `cursor/client.go`).

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::connect::{Client, Error};
use crate::events::{MSG_STATUS, RunStreamEvent, STATUS_ERROR, decode_frame, run_status_terminal};
use crate::gojson::null_default;

const SVC_AGENT: &str = "sdk.v1.SdkAgentService";
const SVC_CURSOR: &str = "sdk.v1.SdkCursorService";
const SVC_CONTROL: &str = "sdk.v1.SdkBridgeControlService";

/// GetVersion. Recur checks the capability list at startup so a bridge too
/// old for what it needs fails by name instead of with an opaque RPC error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VersionInfo {
    #[serde(deserialize_with = "null_default")]
    pub bridge_version: String,
    #[serde(deserialize_with = "null_default")]
    pub protocol_version: String,
    #[serde(deserialize_with = "null_default")]
    pub capabilities: Vec<String>,
}

impl VersionInfo {
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

/// One ListModels entry (SDK ModelListItem). Parameters and variants become
/// the Settings flag dimensions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub display_name: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    #[serde(deserialize_with = "null_default")]
    pub aliases: Vec<String>,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub parameters: Vec<ModelParameterDefinition>,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub variants: Vec<ModelVariant>,
}

/// One configurable dimension (fast, effort, context, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelParameterDefinition {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub display_name: String,
    #[serde(deserialize_with = "null_default")]
    pub values: Vec<ModelParameterChoice>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelParameterChoice {
    #[serde(deserialize_with = "null_default")]
    pub value: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub display_name: String,
}

/// A named preset of parameter values ("High Fast").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelVariant {
    #[serde(deserialize_with = "null_default")]
    pub params: Vec<ModelParameterValue>,
    #[serde(deserialize_with = "null_default")]
    pub display_name: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
    #[serde(deserialize_with = "null_default", skip_serializing_if = "is_false")]
    pub is_default: bool,
}

/// One `{id, value}` pair on a ModelSelection or variant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelParameterValue {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub value: String,
}

/// Me (sdk.v1.SdkUser).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct User {
    #[serde(deserialize_with = "null_default")]
    pub api_key_name: String,
    #[serde(deserialize_with = "null_default")]
    pub user_email: String,
    #[serde(deserialize_with = "null_default")]
    pub user_first_name: String,
    #[serde(deserialize_with = "null_default")]
    pub user_last_name: String,
    #[serde(deserialize_with = "null_default")]
    pub created_at: String,
}

impl User {
    /// First + last name when present.
    pub fn display_name(&self) -> String {
        let first = self.user_first_name.trim();
        let last = self.user_last_name.trim();
        match (first.is_empty(), last.is_empty()) {
            (false, false) => format!("{first} {last}"),
            (false, true) => first.to_owned(),
            _ => last.to_owned(),
        }
    }

    /// A short identity for toasts: email, else key name.
    pub fn summary(&self) -> String {
        let e = self.user_email.trim();
        if !e.is_empty() {
            return e.to_owned();
        }
        self.api_key_name.trim().to_owned()
    }
}

/// One inline `mcpServers` entry. The bridge's oneof is a wrapper object:
/// `{"http":{"type":"HTTP_MCP_TRANSPORT_TYPE_HTTP","url":…}}`. Passing MCP
/// inline is why Recur uses the bridge: the CLI only reads
/// `.cursor/mcp.json` from fixed paths, which would dirty the worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct McpServer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpMcpServer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdio: Option<StdioMcpServer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HttpMcpServer {
    /// HTTP_MCP_TRANSPORT_TYPE_HTTP.
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// The subprocess transport (the QA role's Playwright MCP).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StdioMcpServer {
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
}

impl McpServer {
    /// An inline streamable-HTTP entry. Recur's MCP server puts a per-agent
    /// token in the URL path, so no headers are needed.
    pub fn http(url: &str) -> Self {
        Self {
            http: Some(HttpMcpServer {
                kind: "HTTP_MCP_TRANSPORT_TYPE_HTTP".into(),
                url: url.to_owned(),
                headers: BTreeMap::new(),
            }),
            stdio: None,
        }
    }

    pub fn stdio(command: &str, args: &[&str]) -> Self {
        Self {
            http: None,
            stdio: Some(StdioMcpServer {
                command: command.to_owned(),
                args: args.iter().map(|a| (*a).to_owned()).collect(),
                ..Default::default()
            }),
        }
    }
}

/// `{id, params}`. Params carry per-model flags; omitted ones use each
/// parameter's first allowed value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSelection {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub params: Vec<ModelParameterValue>,
}

/// The local-runtime options on AgentOptions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LocalOptions {
    /// The worktree. `repeated string` on the wire: a bare string is
    /// rejected with `cannot decode field sdk.v1.LocalAgentOptions.cwd`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cwd: Vec<String>,
    /// Ambient Cursor config layers as SETTING_SOURCE_* names. An empty list
    /// does NOT mean "load nothing": the bridge maps length 0 to SDK
    /// defaults. Name the layers you want ([`SETTING_SOURCES_MINIMAL`]).
    #[serde(rename = "settingSources", skip_serializing_if = "Vec::is_empty")]
    pub setting_sources: Vec<String>,
    /// Always sent as `enabled:false`: omitted, a per-user
    /// `~/.cursor/sandbox.json` can turn sandboxing on, which has crashed the
    /// bridge mid Shell tool and blocks the build/test commands agents need.
    #[serde(rename = "sandboxOptions", skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxOptions>,
}

pub const SETTING_SOURCE_PROJECT: &str = "SETTING_SOURCE_PROJECT";
pub const SETTING_SOURCE_USER: &str = "SETTING_SOURCE_USER";
pub const SETTING_SOURCE_TEAM: &str = "SETTING_SOURCE_TEAM";
pub const SETTING_SOURCE_MDM: &str = "SETTING_SOURCE_MDM";
pub const SETTING_SOURCE_PLUGINS: &str = "SETTING_SOURCE_PLUGINS";
pub const SETTING_SOURCE_ALL: &str = "SETTING_SOURCE_ALL";

/// The narrowest layers Recur can ask for: MDM is empty on machines without
/// managed config, so naming only it suppresses user and project config.
/// Not the default (a repo's .cursor/rules and AGENTS.md are project
/// context); used for the strict-config setting.
pub const SETTING_SOURCES_MINIMAL: &[&str] = &[SETTING_SOURCE_MDM];

/// `enabled` is never omitted: the wire value must be `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SandboxOptions {
    pub enabled: bool,
}

/// The sdk.v1 AgentOptions payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(rename = "apiKey", skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalOptions>,
    #[serde(rename = "mcpServers", skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_servers: BTreeMap<String, McpServer>,
    /// "agent" (default) or "plan" (read-only): the closest Cursor offers to
    /// per-role tool scoping.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
}

pub const STEER_COMPLETE_DELIVERED: &str = "complete_delivered";
pub const STEER_REVERT_TO_FOLLOWUP: &str = "revert_to_followup";

/// One ListRuns row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunSnapshot {
    #[serde(deserialize_with = "null_default")]
    pub run_id: String,
    #[serde(deserialize_with = "null_default")]
    pub agent_id: String,
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    pub created_at: ProtoTime,
}

/// google.protobuf.Timestamp JSON: RFC3339 or `{seconds, nanos}`. Invalid
/// values stay unset rather than failing ListRuns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProtoTime(pub Option<DateTime<Utc>>);

impl<'de> Deserialize<'de> for ProtoTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self(match Value::deserialize(d)? {
            Value::String(s) if !s.is_empty() => DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|t| t.with_timezone(&Utc)),
            Value::Object(m) => {
                let seconds = match m.get("seconds") {
                    Some(Value::String(s)) if !s.is_empty() => s.parse::<i64>().ok(),
                    Some(Value::Number(n)) => n.as_i64(),
                    _ => None,
                };
                let nanos = match m.get("nanos") {
                    Some(Value::Number(n)) => n.as_i64().and_then(|n| i32::try_from(n).ok()),
                    None | Some(Value::Null) => Some(0),
                    _ => None,
                };
                match (seconds, nanos) {
                    (Some(s), Some(n)) => DateTime::from_timestamp(s, 0)
                        .and_then(|t| t.checked_add_signed(TimeDelta::nanoseconds(n.into()))),
                    _ => None,
                }
            }
            _ => None,
        }))
    }
}

impl Serialize for ProtoTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            None => s.serialize_unit(),
            Some(t) => s.serialize_str(&rfc3339_nano(t)),
        }
    }
}

/// Go `time.RFC3339Nano` in UTC: fractional seconds with trailing zeros
/// dropped.
pub(crate) fn rfc3339_nano(t: DateTime<Utc>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = t.timestamp_subsec_nanos();
    if nanos == 0 {
        return format!("{base}Z");
    }
    let frac = format!("{nanos:09}");
    format!("{base}.{}Z", frac.trim_end_matches('0'))
}

/// Whether a run's timestamp places it after this bridge process started.
/// Missing timestamps fail closed: ObserveRun on a leftover SQLite row would
/// replay then hang, so unknown runs take the cancel+Send path.
pub fn run_created_on_this_bridge(
    run: &RunSnapshot,
    bridge_started: Option<DateTime<Utc>>,
) -> bool {
    if run.run_id.is_empty() {
        return false;
    }
    match (run.created_at.0, bridge_started) {
        (Some(created), Some(started)) => created >= started - TimeDelta::seconds(1),
        _ => false,
    }
}

/// One image on a user message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Image {
    pub data: String,
    pub mime_type: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn decode<T: DeserializeOwned + Default>(
    raw: &[u8],
    service: &str,
    method: &str,
) -> Result<T, Error> {
    if raw.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice::<Option<T>>(raw)
        .map(Option::unwrap_or_default)
        .map_err(|e| Error::msg(format!("cursor bridge: decode {service}/{method}: {e}")))
}

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct WithKey<'a> {
    options: KeyOnly<'a>,
}

#[derive(Serialize)]
struct KeyOnly<'a> {
    #[serde(rename = "apiKey")]
    api_key: &'a str,
}

impl Client {
    pub async fn get_version(&self, cancel: &CancellationToken) -> Result<VersionInfo, Error> {
        let raw = self
            .unary(cancel, SVC_CONTROL, "GetVersion", &Empty {})
            .await?;
        decode(&raw, SVC_CONTROL, "GetVersion")
    }

    pub async fn ping(&self, cancel: &CancellationToken) -> Result<(), Error> {
        self.unary(cancel, SVC_CONTROL, "Ping", &Empty {})
            .await
            .map(drop)
    }

    pub async fn shutdown(&self, cancel: &CancellationToken) -> Result<(), Error> {
        #[derive(Serialize)]
        struct Req {
            #[serde(rename = "graceSeconds")]
            grace_seconds: i32,
        }
        self.unary(cancel, SVC_CONTROL, "Shutdown", &Req { grace_seconds: 0 })
            .await
            .map(drop)
    }

    /// The models the key may use; also the cheapest credential check.
    pub async fn list_models(
        &self,
        cancel: &CancellationToken,
        api_key: &str,
    ) -> Result<Vec<Model>, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "null_default")]
            items: Vec<Model>,
        }
        let req = WithKey {
            options: KeyOnly { api_key },
        };
        let raw = self.unary(cancel, SVC_CURSOR, "ListModels", &req).await?;
        Ok(decode::<Out>(&raw, SVC_CURSOR, "ListModels")?.items)
    }

    pub async fn me(&self, cancel: &CancellationToken, api_key: &str) -> Result<User, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "null_default")]
            user: User,
        }
        let req = WithKey {
            options: KeyOnly { api_key },
        };
        let raw = self.unary(cancel, SVC_CURSOR, "Me", &req).await?;
        Ok(decode::<Out>(&raw, SVC_CURSOR, "Me")?.user)
    }

    /// Creates a local agent and returns its id.
    pub async fn create_agent(
        &self,
        cancel: &CancellationToken,
        opts: &AgentOptions,
    ) -> Result<String, Error> {
        #[derive(Serialize)]
        struct Req<'a> {
            options: &'a AgentOptions,
        }
        let raw = self
            .unary(cancel, SVC_AGENT, "CreateAgent", &Req { options: opts })
            .await?;
        Ok(decode::<AgentIdOut>(&raw, SVC_AGENT, "CreateAgent")?.agent_id)
    }

    /// Reattaches to an earlier agent, keeping its conversation: Recur's
    /// multi-turn continuity (the agent id is the stored resume handle).
    pub async fn resume_agent(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
        opts: &AgentOptions,
    ) -> Result<String, Error> {
        #[derive(Serialize)]
        struct Req<'a> {
            #[serde(rename = "agentId")]
            agent_id: &'a str,
            options: &'a AgentOptions,
        }
        let raw = self
            .unary(
                cancel,
                SVC_AGENT,
                "ResumeAgent",
                &Req {
                    agent_id,
                    options: opts,
                },
            )
            .await?;
        let out = decode::<AgentIdOut>(&raw, SVC_AGENT, "ResumeAgent")?;
        Ok(if out.agent_id.is_empty() {
            agent_id.to_owned()
        } else {
            out.agent_id
        })
    }

    /// Releases bridge-side agent state. Best-effort: a failure leaks a
    /// little bridge memory but must never fail a run.
    pub async fn close_agent(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
    ) -> Result<(), Error> {
        self.unary(cancel, SVC_AGENT, "CloseAgent", &AgentIdReq { agent_id })
            .await
            .map(drop)
    }

    /// Stops an in-flight run.
    pub async fn cancel_run(&self, cancel: &CancellationToken, run_id: &str) -> Result<(), Error> {
        self.unary(cancel, SVC_AGENT, "CancelRun", &RunIdReq { run_id })
            .await
            .map(drop)
    }

    /// Injects text into a live run (SDK run.steer). `complete_delivered`
    /// means the turn has the message; anything else, including a missing
    /// RPC, means the caller should Send after the run instead. Follows the
    /// token through terminal acknowledgement, not the unary timeout.
    pub async fn steer_run(
        &self,
        cancel: &CancellationToken,
        run_id: &str,
        text: &str,
    ) -> Result<String, Error> {
        #[derive(Serialize)]
        struct Req<'a> {
            #[serde(rename = "runId")]
            run_id: &'a str,
            text: &'a str,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "null_default")]
            outcome: String,
        }
        let raw = self
            .unary_ctx(cancel, SVC_AGENT, "SteerRun", &Req { run_id, text })
            .await?;
        Ok(decode::<Out>(&raw, SVC_AGENT, "SteerRun")?.outcome)
    }

    /// Runs for an agent, newest first.
    pub async fn list_runs(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
    ) -> Result<Vec<RunSnapshot>, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "null_default")]
            items: Vec<RunSnapshot>,
        }
        let raw = self
            .unary(cancel, SVC_AGENT, "ListRuns", &AgentIdReq { agent_id })
            .await?;
        Ok(decode::<Out>(&raw, SVC_AGENT, "ListRuns")?.items)
    }

    /// The newest creating/running run; a default snapshot means none.
    pub async fn first_non_terminal_run(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
    ) -> Result<RunSnapshot, Error> {
        let runs = self.list_runs(cancel, agent_id).await?;
        Ok(runs
            .into_iter()
            .find(|r| !r.run_id.is_empty() && !run_status_terminal(&r.status))
            .unwrap_or_default())
    }

    /// Cancels every creating/running run: the fallback when ObserveRun
    /// cannot re-attach after "already has active run". Returns how many
    /// were cancelled and the first failure.
    pub async fn cancel_non_terminal_runs(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
    ) -> Result<usize, Error> {
        let runs = self.list_runs(cancel, agent_id).await?;
        let mut n = 0;
        let mut first = None;
        for run in runs {
            if run.run_id.is_empty() || run_status_terminal(&run.status) {
                continue;
            }
            match self.cancel_run(cancel, &run.run_id).await {
                Ok(()) => n += 1,
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        match first {
            Some(e) => Err(e),
            None => Ok(n),
        }
    }

    /// Starts a run and streams its frames. Images ride as protobuf-JSON
    /// `{"data":{"data":…,"mimeType":…}}` (SdkImage is a oneof); the flat
    /// TypeScript shape is dropped by the bridge.
    pub async fn send(
        &self,
        cancel: &CancellationToken,
        agent_id: &str,
        text: &str,
        images: &[Image],
        mut on_event: impl FnMut(RunStreamEvent),
    ) -> Result<(), Error> {
        #[derive(Serialize)]
        struct ImageData<'a> {
            data: &'a str,
            #[serde(rename = "mimeType")]
            mime_type: &'a str,
        }
        #[derive(Serialize)]
        struct WireImage<'a> {
            data: ImageData<'a>,
        }
        #[derive(Serialize)]
        struct Message<'a> {
            #[serde(skip_serializing_if = "Vec::is_empty")]
            images: Vec<WireImage<'a>>,
            text: &'a str,
        }
        #[derive(Serialize)]
        struct Options {
            /// Attaches onDelta: summary, summary-started, summary-completed
            /// and turn-ended (the summary members need Recur's minify patch).
            #[serde(rename = "enableDeltas")]
            enable_deltas: bool,
        }
        #[derive(Serialize)]
        struct Req<'a> {
            #[serde(rename = "agentId")]
            agent_id: &'a str,
            message: Message<'a>,
            options: Options,
        }
        let req = Req {
            agent_id,
            message: Message {
                images: images
                    .iter()
                    .map(|i| WireImage {
                        data: ImageData {
                            data: &i.data,
                            mime_type: &i.mime_type,
                        },
                    })
                    .collect(),
                text,
            },
            options: Options {
                enable_deltas: true,
            },
        };
        self.stream(cancel, SVC_AGENT, "Send", &req, |raw| {
            self.decode_run_stream(raw, &mut on_event);
            Ok(())
        })
        .await
    }

    /// Subscribes to an existing run (same frames as Send). `after_offset` is
    /// exclusive; empty starts from the beginning of durable events.
    pub async fn observe_run(
        &self,
        cancel: &CancellationToken,
        run_id: &str,
        after_offset: &str,
        mut on_event: impl FnMut(RunStreamEvent),
    ) -> Result<(), Error> {
        #[derive(Serialize)]
        struct Req<'a> {
            #[serde(rename = "afterOffset", skip_serializing_if = "str::is_empty")]
            after_offset: &'a str,
            #[serde(rename = "runId")]
            run_id: &'a str,
        }
        self.stream(
            cancel,
            SVC_AGENT,
            "ObserveRun",
            &Req {
                after_offset,
                run_id,
            },
            |raw| {
                self.decode_run_stream(raw, &mut on_event);
                Ok(())
            },
        )
        .await
    }

    fn decode_run_stream(&self, raw: &[u8], on_event: &mut impl FnMut(RunStreamEvent)) {
        // One unparseable frame must not kill a run.
        let Some(ev) = decode_frame(raw) else {
            return;
        };
        // The bridge never logs a failed run and its proto RunResult drops
        // the SDK's RunError: this status frame is the only trace of why.
        if let Some(m) = &ev.sdk_message
            && m.kind == MSG_STATUS
            && m.status == STATUS_ERROR
            && !m.message.is_empty()
        {
            self.note(&format!(
                "recur: run {} status ERROR: {}",
                m.run_id, m.message
            ));
        }
        on_event(ev);
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AgentIdOut {
    #[serde(rename = "agentId", deserialize_with = "null_default")]
    agent_id: String,
}

#[derive(Serialize)]
struct AgentIdReq<'a> {
    #[serde(rename = "agentId")]
    agent_id: &'a str,
}

#[derive(Serialize)]
struct RunIdReq<'a> {
    #[serde(rename = "runId")]
    run_id: &'a str,
}

#[cfg(test)]
mod tests;
