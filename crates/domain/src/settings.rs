//! Machine-global caps and agent/provider config (PRD §18).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::{ModelRef, PROVIDER_CLAUDE_CODE, parse_model_ref};
use crate::serde_util::{is_false, is_zero};
use crate::session::{
    CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW, normalize_chat_mode,
};
use crate::skills::name_enabled;
use crate::{
    CompatibleModel, CompatibleProvider, McpServer, MergeCommitMessage, MergeMethod,
    SquashCommitMessage, UpdateBaseMethod,
};

/// Flags for one model slot: dimension → value, e.g. {"effort":"high"}.
pub type Flags = BTreeMap<String, String>;

string_enum! {
    /// The legacy (pre-string) branch-prefix schema, still read when
    /// `Settings::branch_name_prefix` is unset.
    BranchPrefixMode {
        Recur = "recur",
        GitHub = "github",
        Custom = "custom",
        None = "none",
    }
}

/// The historical workspace branch namespace.
pub const DEFAULT_BRANCH_NAME_PREFIX: &str = "recur";

/// Shipped Linear bundle `[[setting]]` ids, read via RECUR_SETTING_*.
pub const SETTING_LINEAR_API_KEY: &str = "recur.linear.api-key";
pub const SETTING_LINEAR_TEAM_KEY: &str = "recur.linear.team-key";

/// The shipped interactive agent turn deadline: 4 hours.
pub const DEFAULT_AGENT_TURN_TIMEOUT_MINUTES: i64 = 4 * 60;
/// Avoids accidental 1-minute kills.
pub const MIN_AGENT_TURN_TIMEOUT_MINUTES: i64 = 5;
/// 7 days.
pub const MAX_AGENT_TURN_TIMEOUT_MINUTES: i64 = 7 * 24 * 60;

pub const DEFAULT_MAX_LOOPS: i64 = 4;
pub const DEFAULT_MAX_AGENTS: i64 = 8;

// Claude Code --permission-mode values. Ask is paired with
// --permission-prompt-tool; Deny is the fallback for turns with no MCP
// endpoint to route a prompt through.
pub const PERMISSION_MODE_BYPASS: &str = "bypassPermissions";
pub const PERMISSION_MODE_ASK: &str = "default";
pub const PERMISSION_MODE_DENY: &str = "dontAsk";

// How create_workspace picks the spawned tab's model and flags.
pub const CHILD_WORKSPACE_MODEL_PARENT: &str = "parent";
pub const CHILD_WORKSPACE_MODEL_MODE: &str = "mode";

const DEFAULT_WORKSPACE_TITLE_MODEL: &str = "claude-code/claude-haiku-4-5-20251001";

/// Machine-global caps and agent/provider config.
///
/// Per-mode models are fully-qualified refs (`provider/id`), one slot per
/// chat mode; the provider prefix routes the turn. Loop specialists reuse the
/// slots: Maestro←Loop, Coder←Agent, Reviewer←Review (empty Review falls back
/// to Loop).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub model_ask: String,
    pub model_plan: String,
    pub model_agent: String,
    pub model_review: String,
    pub model_loop: String,
    /// The cheap one-shot model for AI commit messages. Empty → Claude Haiku.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_workspace_title: String,
    /// Per-mode model flags. Empty → provider defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_ask: Flags,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_plan: Flags,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_agent: Flags,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_review: Flags,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_loop: Flags,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags_workspace_title: Flags,
    /// Empty or "parent": copy the calling chat. "mode": Settings → Models
    /// for the child's chat mode.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub child_workspace_model: String,
    /// The machine-global permission switch for EVERY agent turn. Off (the
    /// default), tools that need approval reach the human; on, nothing is
    /// ever asked about.
    pub bypass_permissions: bool,
    /// Concurrency caps (PRD §6.4). Zero → unlimited. Negative → defaults.
    pub max_loops: i64,
    pub max_agents: i64,
    /// Empty → merge commit (D53).
    #[serde(skip_serializing_if = "MergeMethod::is_empty")]
    pub default_merge_method: MergeMethod,
    /// Empty → merge (D54).
    #[serde(skip_serializing_if = "UpdateBaseMethod::is_empty")]
    pub default_update_base_method: UpdateBaseMethod,
    /// Empty → title-and-commits (D65).
    #[serde(skip_serializing_if = "SquashCommitMessage::is_empty")]
    pub default_squash_commit_message: SquashCommitMessage,
    /// Empty → title (D65).
    #[serde(skip_serializing_if = "MergeCommitMessage::is_empty")]
    pub default_merge_commit_message: MergeCommitMessage,

    /// Prepended to new workspace branches as prefix/slug. `None` (legacy
    /// blobs) falls back to the mode fields; `Some("")` means no prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name_prefix: Option<String>,
    /// The pre-string schema, read only when branch_name_prefix is unset.
    #[serde(skip_serializing_if = "BranchPrefixMode::is_empty")]
    pub branch_prefix_mode: BranchPrefixMode,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch_prefix_custom: String,

    /// Authenticates Cursor runs. Minted by Recur Connect; cleared by
    /// Disconnect, turning Cursor off, or a missing Connect file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor_api_key: String,
    /// The launcher path last written by Install (display/history only).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor_bridge_path: String,
    /// Suppresses ambient Cursor configuration for agent runs.
    #[serde(skip_serializing_if = "is_false")]
    pub cursor_strict_config: bool,
    /// Lets Cursor agents add the "Made with Cursor" commit trailer.
    #[serde(skip_serializing_if = "is_false")]
    pub cursor_attribute_commits: bool,
    /// Lets Cursor agents add attribution to PRs they open via gh.
    #[serde(rename = "cursorAttributePRs", skip_serializing_if = "is_false")]
    pub cursor_attribute_prs: bool,

    /// `[[setting]]` values keyed by setting id, injected into that bundle's
    /// shebang scripts as RECUR_SETTING_<name>.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bundle_values: BTreeMap<String, String>,

    /// Deprecated aliases migrated into bundle_values on read/write.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub linear_api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub linear_team_key: String,

    /// Hold a macOS `caffeinate -i` assertion while any agent slot is in use.
    #[serde(skip_serializing_if = "is_false")]
    pub prevent_sleep_while_agents_run: bool,
    /// Reveal hidden projects in the sidebar tree.
    #[serde(skip_serializing_if = "is_false")]
    pub show_hidden_projects: bool,
    /// Group the sidebar's workspaces by PR status instead of by project.
    #[serde(skip_serializing_if = "is_false")]
    pub sidebar_by_status: bool,
    /// Linux: keep the window manager's title bar instead of Recur's own window controls.
    /// Read at launch, so a change applies after a restart.
    #[serde(skip_serializing_if = "is_false")]
    pub use_native_title_bar: bool,
    /// ⌘/Ctrl + scroll no longer zooms the UI.
    #[serde(skip_serializing_if = "is_false")]
    pub disable_scroll_zoom: bool,
    /// Enter queues a follow-up while an agent runs (⌘/Ctrl+Enter steers);
    /// default false is the reverse.
    #[serde(skip_serializing_if = "is_false")]
    pub queue_follow_ups: bool,
    /// PR automations: on an open PR with a Recur workspace whose chat is
    /// idle, the sweep runs Update with base (then /update) on merge
    /// conflicts, /fix-checks on failing checks, and /address-review on new
    /// review activity. `None` → on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_update_conflicts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_fix_checks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_address_review: Option<bool>,
    /// The wall-clock ceiling for one interactive agent chat turn; see
    /// [`effective_agent_turn_timeout_minutes`].
    #[serde(skip_serializing_if = "is_zero")]
    pub agent_turn_timeout_minutes: i64,
    /// Live workspaces kept per schedule (older settled runs are removed).
    /// Zero keeps all.
    #[serde(skip_serializing_if = "is_zero")]
    pub scheduled_workspace_keep: i64,

    /// Authenticates Codex runs via CODEX_API_KEY. Kept when Codex is off.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub codex_api_key: String,
    /// Lets Codex use a ChatGPT login.
    #[serde(rename = "codexChatGPTAuth", skip_serializing_if = "is_false")]
    pub codex_chat_gpt_auth: bool,
    /// A `codex` binary. Empty → $RECUR_CODEX, PATH, then Homebrew paths.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub codex_bin_path: String,
    /// Lets Codex Agent shell commands use the network inside the
    /// workspace-write sandbox. Ignored when bypass is on.
    #[serde(skip_serializing_if = "is_false")]
    pub codex_network_access: bool,

    /// A saved ChatGPT account. Tokens are kept separately in
    /// `chatgpt-auth.json` and are never returned to the UI.
    #[serde(rename = "chatGPTAccountId", skip_serializing_if = "String::is_empty")]
    pub chat_gpt_account_id: String,
    #[serde(rename = "chatGPTModels", skip_serializing_if = "Vec::is_empty")]
    pub chat_gpt_models: Vec<CompatibleModel>,

    /// Settings → Bundles off-lists. Empty → all enabled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_commands: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_prompts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_grammars: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_themes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_snippets: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_language_servers: Vec<String>,
    /// Empty → recur.loops.pr.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default_loop: String,
    /// Empty → recur.policies.balanced.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default_policy: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_loops: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_roles: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_bundle_policies: Vec<String>,

    /// Agent Skill names turned off globally (combined with project
    /// overrides at runtime).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_skills: Vec<String>,

    /// The machine catalog of extra MCP servers (Settings → MCP).
    #[serde(rename = "mcpServers", skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,

    /// "Write less" mode. `None` → on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simplify: Option<bool>,

    /// Provider slugs turned off in Settings → Providers. Credentials stay
    /// saved; the provider leaves pickers and the engine will not start it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_providers: Vec<String>,

    /// User-defined OpenAI-compatible endpoints.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub compatible_providers: Vec<CompatibleProvider>,
}

/// The credential values [`Settings`] keeps out of recur.db: the provider
/// keys, every bundle `[[setting]]` value, and the free-form header and env
/// maps (which often carry tokens), keyed by provider or server id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Secrets {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor_api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub codex_api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub linear_api_key: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bundle_values: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub compatible_api_keys: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub compatible_headers: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_headers: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_env: BTreeMap<String, BTreeMap<String, String>>,
}

impl Secrets {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Overlays `newer`'s non-empty values, keeping entries it lacks.
    pub fn merge(&mut self, newer: Secrets) {
        for (old, new) in [
            (&mut self.cursor_api_key, newer.cursor_api_key),
            (&mut self.codex_api_key, newer.codex_api_key),
            (&mut self.linear_api_key, newer.linear_api_key),
        ] {
            if !new.is_empty() {
                *old = new;
            }
        }
        self.bundle_values.extend(newer.bundle_values);
        self.compatible_api_keys.extend(newer.compatible_api_keys);
        self.compatible_headers.extend(newer.compatible_headers);
        self.mcp_headers.extend(newer.mcp_headers);
        self.mcp_env.extend(newer.mcp_env);
    }
}

/// Moves non-empty values into `out` under `id`.
fn take_into<V: Default + PartialEq>(out: &mut BTreeMap<String, V>, id: &str, v: &mut V) {
    let v = std::mem::take(v);
    if v != V::default() {
        out.insert(id.to_owned(), v);
    }
}

impl Settings {
    /// Moves every [`Secrets`] value out, leaving the rest safe to persist.
    pub fn take_secrets(&mut self) -> Secrets {
        let mut s = Secrets {
            cursor_api_key: std::mem::take(&mut self.cursor_api_key),
            codex_api_key: std::mem::take(&mut self.codex_api_key),
            linear_api_key: std::mem::take(&mut self.linear_api_key),
            bundle_values: std::mem::take(&mut self.bundle_values),
            ..Secrets::default()
        };
        for p in &mut self.compatible_providers {
            take_into(&mut s.compatible_api_keys, &p.id, &mut p.api_key);
            take_into(&mut s.compatible_headers, &p.id, &mut p.extra_headers);
        }
        for m in &mut self.mcp_servers {
            take_into(&mut s.mcp_headers, &m.id, &mut m.headers);
            take_into(&mut s.mcp_env, &m.id, &mut m.env);
        }
        s
    }

    /// Restores values taken by [`Settings::take_secrets`]. Entries for ids
    /// no longer in the catalogs are dropped.
    pub fn put_secrets(&mut self, mut s: Secrets) {
        self.cursor_api_key = s.cursor_api_key;
        self.codex_api_key = s.codex_api_key;
        self.linear_api_key = s.linear_api_key;
        self.bundle_values = s.bundle_values;
        for p in &mut self.compatible_providers {
            p.api_key = s.compatible_api_keys.remove(&p.id).unwrap_or_default();
            p.extra_headers = s.compatible_headers.remove(&p.id).unwrap_or_default();
        }
        for m in &mut self.mcp_servers {
            m.headers = s.mcp_headers.remove(&m.id).unwrap_or_default();
            m.env = s.mcp_env.remove(&m.id).unwrap_or_default();
        }
    }

    /// Every [`config_keys`] key with its value (`recur config list`).
    /// Secrets are left out; keys the stored document omits read as their
    /// empty value.
    pub fn config_values(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut public = self.clone();
        public.take_secrets();
        let doc = settings_doc(&public);
        config_keys()
            .into_iter()
            .map(|k| {
                let v = doc.get(k).cloned().unwrap_or_else(|| empty_value(&doc, k));
                (k, v)
            })
            .collect()
    }

    /// A copy with config `key` set to `value`, or why that does not fit.
    /// Secrets carry over and cannot be set this way.
    pub fn with_config_value(&self, key: &str, value: serde_json::Value) -> Result<Self, String> {
        if !config_keys().contains(&key) {
            if struct_fields::<Secrets>().contains(&key) {
                return Err(format!("{key} is a secret: set it in Recur's Settings"));
            }
            return Err(format!("unknown setting {key:?}"));
        }
        let mut cur = self.clone();
        let secrets = cur.take_secrets();
        let mut doc = settings_doc(&cur);
        doc.insert(key.to_owned(), value);
        let mut next: Self = serde_json::from_value(serde_json::Value::Object(doc))
            .map_err(|e| format!("invalid value for {key}: {e}"))?;
        crate::validate_mcp_servers(&next.mcp_servers)
            .map_err(|e| format!("invalid value for {key}: {e}"))?;
        crate::validate_compatible_providers(&next.compatible_providers)
            .map_err(|e| format!("invalid value for {key}: {e}"))?;
        next.take_secrets();
        next.put_secrets(secrets);
        Ok(next)
    }
}

/// The stored settings keys `recur config` reads and writes: every
/// [`Settings`] key except the [`Secrets`].
pub fn config_keys() -> Vec<&'static str> {
    let secret = struct_fields::<Secrets>();
    struct_fields::<Settings>()
        .iter()
        .copied()
        .filter(|k| !secret.contains(k))
        .collect()
}

fn settings_doc(s: &Settings) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(s) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    }
}

/// The value of a key the document skips as empty: the first candidate that
/// deserializes and is skipped again on the way out.
fn empty_value(doc: &serde_json::Map<String, serde_json::Value>, key: &str) -> serde_json::Value {
    use serde_json::{Value, json};
    [
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({}),
    ]
    .into_iter()
    .find(|v| {
        let mut d = doc.clone();
        d.insert(key.to_owned(), v.clone());
        serde_json::from_value::<Settings>(Value::Object(d))
            .is_ok_and(|s| !settings_doc(&s).contains_key(key))
    })
    .unwrap_or(Value::Null)
}

/// The field names a derived `Deserialize` asks for, recorded by a
/// deserializer that stops at `deserialize_struct`.
fn struct_fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    use serde::de::{Error, Visitor, value};

    struct Probe<'a>(&'a mut &'static [&'static str]);

    impl<'de> serde::Deserializer<'de> for Probe<'_> {
        type Error = value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, value::Error> {
            Err(value::Error::custom("not a struct"))
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, value::Error> {
            *self.0 = fields;
            Err(value::Error::custom("probed"))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }

    let mut fields: &'static [&'static str] = &[];
    let _ = T::deserialize(Probe(&mut fields));
    fields
}

/// Normalizes a turn deadline in minutes: non-positive → default, otherwise
/// clamped to [min, max].
pub fn effective_agent_turn_timeout_minutes(minutes: i64) -> i64 {
    if minutes <= 0 {
        return DEFAULT_AGENT_TURN_TIMEOUT_MINUTES;
    }
    minutes.clamp(
        MIN_AGENT_TURN_TIMEOUT_MINUTES,
        MAX_AGENT_TURN_TIMEOUT_MINUTES,
    )
}

/// Normalizes the loop cap: negative → default, zero is unlimited, positive
/// passes through with no upper bound.
pub fn effective_max_loops(n: i64) -> i64 {
    if n < 0 { DEFAULT_MAX_LOOPS } else { n }
}

/// Normalizes the agent cap like [`effective_max_loops`].
pub fn effective_max_agents(n: i64) -> i64 {
    if n < 0 { DEFAULT_MAX_AGENTS } else { n }
}

impl Settings {
    /// Copies deprecated linear_api_key / linear_team_key into bundle_values
    /// (existing non-blank values win) and clears the old fields.
    pub fn migrate_bundle_values(mut self) -> Self {
        for (legacy, id) in [
            (&mut self.linear_api_key, SETTING_LINEAR_API_KEY),
            (&mut self.linear_team_key, SETTING_LINEAR_TEAM_KEY),
        ] {
            let k = legacy.trim();
            if k.is_empty() {
                continue;
            }
            if self
                .bundle_values
                .get(id)
                .is_none_or(|v| v.trim().is_empty())
            {
                self.bundle_values.insert(id.to_owned(), k.to_owned());
            }
            legacy.clear();
        }
        self
    }

    /// The stored `[[setting]]` value for id after Linear migration, trimmed.
    pub fn bundle_value(&self, id: &str) -> String {
        let stored = self.bundle_values.get(id).map_or("", |v| v.trim());
        if !stored.is_empty() {
            return stored.to_owned();
        }
        match id {
            SETTING_LINEAR_API_KEY => self.linear_api_key.trim().to_owned(),
            SETTING_LINEAR_TEAM_KEY => self.linear_team_key.trim().to_owned(),
            _ => String::new(),
        }
    }

    /// The wall-clock limit for one interactive agent chat turn.
    pub fn agent_turn_timeout(&self) -> Duration {
        let minutes = effective_agent_turn_timeout_minutes(self.agent_turn_timeout_minutes);
        Duration::from_secs(minutes as u64 * 60)
    }

    /// Whether name is not in disabled_skills.
    pub fn skill_enabled(&self, name: &str) -> bool {
        name_enabled(name, &self.disabled_skills)
    }

    /// Whether Simplify mode is on. Unset → on.
    pub fn simplify_enabled(&self) -> bool {
        self.simplify.unwrap_or(true)
    }

    /// Whether the sweep resolves a PR's merge conflicts. Unset → on.
    pub fn auto_update_conflicts_on(&self) -> bool {
        self.auto_update_conflicts.unwrap_or(true)
    }

    /// Whether the sweep runs /fix-checks on a PR's failing checks. Unset → on.
    pub fn auto_fix_checks_on(&self) -> bool {
        self.auto_fix_checks.unwrap_or(true)
    }

    /// Whether the sweep runs /address-review on new review activity. Unset → on.
    pub fn auto_address_review_on(&self) -> bool {
        self.auto_address_review.unwrap_or(true)
    }

    /// Whether Cursor credentials are present.
    pub fn cursor_configured(&self) -> bool {
        !self.cursor_api_key.trim().is_empty()
    }

    /// Whether Codex credentials are present (an API key or an explicit
    /// ChatGPT-auth opt-in).
    pub fn codex_configured(&self) -> bool {
        !self.codex_api_key.trim().is_empty() || self.codex_chat_gpt_auth
    }

    /// Whether any mode (or workspace-title) model points at provider.
    pub fn uses_provider(&self, provider: &str) -> bool {
        [
            self.ask_model(),
            self.plan_model(),
            self.agent_model(),
            self.review_model(),
            self.loop_model(),
            self.workspace_title_model(),
        ]
        .iter()
        .any(|m| m.provider == provider)
    }

    /// The Claude Code --permission-mode for every agent turn. Off maps to
    /// "default" (not "dontAsk") because default actually raises a request.
    pub fn permission_mode(&self) -> &'static str {
        if self.bypass_permissions {
            PERMISSION_MODE_BYPASS
        } else {
            PERMISSION_MODE_ASK
        }
    }

    pub fn ask_model(&self) -> ModelRef {
        parse_model_ref(&self.model_ask)
    }

    pub fn plan_model(&self) -> ModelRef {
        parse_model_ref(&self.model_plan)
    }

    pub fn agent_model(&self) -> ModelRef {
        parse_model_ref(&self.model_agent)
    }

    /// Review mode's model, falling back to Loop when unset so a
    /// half-configured Settings pane still runs.
    pub fn review_model(&self) -> ModelRef {
        if self.model_review.trim().is_empty() {
            return self.loop_model();
        }
        parse_model_ref(&self.model_review)
    }

    pub fn loop_model(&self) -> ModelRef {
        parse_model_ref(&self.model_loop)
    }

    /// The loop orchestrator's model (Loop mode).
    pub fn maestro_model(&self) -> ModelRef {
        self.loop_model()
    }

    /// The loop coder's model (Agent mode).
    pub fn coder_model(&self) -> ModelRef {
        self.agent_model()
    }

    /// The loop reviewer's model (Review mode, with Loop fallback).
    pub fn reviewer_model(&self) -> ModelRef {
        self.review_model()
    }

    /// The cheap completion model for AI commit messages and similar
    /// one-shot titles. Empty setting → Claude Haiku.
    pub fn workspace_title_model(&self) -> ModelRef {
        if self.model_workspace_title.trim().is_empty() {
            return parse_model_ref(DEFAULT_WORKSPACE_TITLE_MODEL);
        }
        parse_model_ref(&self.model_workspace_title)
    }

    pub fn ask_flags(&self) -> Flags {
        self.flags_ask.clone()
    }

    pub fn plan_flags(&self) -> Flags {
        self.flags_plan.clone()
    }

    pub fn agent_flags(&self) -> Flags {
        self.flags_agent.clone()
    }

    pub fn review_flags(&self) -> Flags {
        self.flags_review.clone()
    }

    pub fn loop_flags(&self) -> Flags {
        self.flags_loop.clone()
    }

    pub fn workspace_title_flags(&self) -> Flags {
        self.flags_workspace_title.clone()
    }

    pub fn maestro_flags(&self) -> Flags {
        self.loop_flags()
    }

    pub fn coder_flags(&self) -> Flags {
        self.agent_flags()
    }

    pub fn reviewer_flags(&self) -> Flags {
        self.review_flags()
    }

    /// The default model ref (storage form) + flags for a chat mode when the
    /// dock has not overridden them. Unknown modes are agent.
    pub fn model_for_chat_mode(&self, mode: &str) -> (String, Flags) {
        match normalize_chat_mode(mode) {
            CHAT_MODE_ASK => (self.ask_model().to_string(), self.ask_flags()),
            CHAT_MODE_PLAN => (self.plan_model().to_string(), self.plan_flags()),
            CHAT_MODE_REVIEW => (self.review_model().to_string(), self.review_flags()),
            CHAT_MODE_LOOP => (self.loop_model().to_string(), self.loop_flags()),
            _ => (self.agent_model().to_string(), self.agent_flags()),
        }
    }

    /// How create_workspace picks the spawned tab's model and flags. Empty or
    /// unknown → parent (copy the calling chat).
    pub fn effective_child_workspace_model(&self) -> &'static str {
        if self.child_workspace_model.trim() == CHILD_WORKSPACE_MODEL_MODE {
            CHILD_WORKSPACE_MODEL_MODE
        } else {
            CHILD_WORKSPACE_MODEL_PARENT
        }
    }

    /// The path segment prepended to new workspace branches (`prefix/slug`).
    /// Empty means no prefix. The stored string wins; the legacy mode fields
    /// apply only when it is unset (an unmigrated "github" is not looked up).
    pub fn effective_branch_name_prefix(&self) -> String {
        if let Some(p) = &self.branch_name_prefix {
            return p.trim().to_owned();
        }
        match self.branch_prefix_mode {
            BranchPrefixMode::None => String::new(),
            BranchPrefixMode::Custom if !self.branch_prefix_custom.trim().is_empty() => {
                self.branch_prefix_custom.trim().to_owned()
            }
            _ => DEFAULT_BRANCH_NAME_PREFIX.to_owned(),
        }
    }

    /// The provider is the default (Claude Code) when the slug is empty.
    pub(crate) fn default_provider(provider: &str) -> &str {
        if provider.is_empty() {
            PROVIDER_CLAUDE_CODE
        } else {
            provider
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keys_values_and_set() {
        let keys = config_keys();
        assert!(keys.contains(&"modelAgent") && keys.contains(&"cursorAttributePRs"));
        assert!(!keys.contains(&"cursorApiKey") && !keys.contains(&"bundleValues"));

        let mut set = Settings {
            cursor_api_key: "sk".into(),
            ..Settings::default()
        };
        set.bundle_values.insert("a".into(), "b".into());
        let values: BTreeMap<_, _> = set.config_values().into_iter().collect();
        assert_eq!(values.len(), keys.len());
        assert_eq!(values["simplify"], serde_json::Value::Null);
        assert_eq!(values["maxAgents"], serde_json::json!(0));
        assert_eq!(values["queueFollowUps"], serde_json::json!(false));
        assert_eq!(values["disabledSkills"], serde_json::json!([]));
        assert_eq!(values["flagsAgent"], serde_json::json!({}));

        let next = set
            .with_config_value("flagsAgent", serde_json::json!({"effort": "high"}))
            .unwrap();
        assert_eq!(next.flags_agent, flags(&[("effort", "high")]));
        assert_eq!(
            (next.cursor_api_key.as_str(), &next.bundle_values),
            ("sk", &set.bundle_values)
        );
        assert!(set.with_config_value("maxAgents", "x".into()).is_err());
        assert!(
            set.with_config_value(
                "mcpServers",
                serde_json::json!([{"id": "playwright", "command": "npx"}])
            )
            .unwrap_err()
            .contains("reserved")
        );
        assert!(
            set.with_config_value("nope", 1.into())
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            set.with_config_value("cursorApiKey", "x".into())
                .unwrap_err()
                .contains("secret")
        );
    }

    fn flags(pairs: &[(&str, &str)]) -> Flags {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn branch_name_prefix() {
        let cases = [
            (
                "unset is recur",
                Settings::default(),
                DEFAULT_BRANCH_NAME_PREFIX,
            ),
            (
                "stored recur",
                Settings {
                    branch_name_prefix: Some("recur".into()),
                    ..Default::default()
                },
                "recur",
            ),
            (
                "stored custom",
                Settings {
                    branch_name_prefix: Some("josh".into()),
                    ..Default::default()
                },
                "josh",
            ),
            (
                "stored empty is none",
                Settings {
                    branch_name_prefix: Some(String::new()),
                    ..Default::default()
                },
                "",
            ),
            (
                "stored trims",
                Settings {
                    branch_name_prefix: Some("  team  ".into()),
                    ..Default::default()
                },
                "team",
            ),
            (
                "legacy none",
                Settings {
                    branch_prefix_mode: BranchPrefixMode::None,
                    ..Default::default()
                },
                "",
            ),
            (
                "legacy custom",
                Settings {
                    branch_prefix_mode: BranchPrefixMode::Custom,
                    branch_prefix_custom: "josh".into(),
                    ..Default::default()
                },
                "josh",
            ),
            (
                "legacy custom blank",
                Settings {
                    branch_prefix_mode: BranchPrefixMode::Custom,
                    ..Default::default()
                },
                "recur",
            ),
            (
                "legacy github is recur without a stored string",
                Settings {
                    branch_prefix_mode: BranchPrefixMode::GitHub,
                    ..Default::default()
                },
                "recur",
            ),
            (
                "string wins over legacy mode",
                Settings {
                    branch_name_prefix: Some("alice".into()),
                    branch_prefix_mode: BranchPrefixMode::None,
                    ..Default::default()
                },
                "alice",
            ),
        ];
        for (name, s, want) in cases {
            assert_eq!(s.effective_branch_name_prefix(), want, "{name}");
        }
    }

    #[test]
    fn branch_name_prefix_json() {
        let s: Settings = serde_json::from_str(r#"{"branchNamePrefix":""}"#).unwrap();
        assert_eq!(s.branch_name_prefix.as_deref(), Some(""));
        assert_eq!(serde_json::to_value(&s).unwrap()["branchNamePrefix"], "");
        let unset: Settings = serde_json::from_str("{}").unwrap();
        assert!(
            serde_json::to_value(&unset)
                .unwrap()
                .get("branchNamePrefix")
                .is_none()
        );
    }

    #[test]
    fn migrate_bundle_values() {
        let s = Settings {
            linear_api_key: "lin_old".into(),
            linear_team_key: "ENG".into(),
            ..Default::default()
        };
        let got = s.migrate_bundle_values();
        assert!(got.linear_api_key.is_empty() && got.linear_team_key.is_empty());
        assert_eq!(got.bundle_values[SETTING_LINEAR_API_KEY], "lin_old");
        assert_eq!(got.bundle_values[SETTING_LINEAR_TEAM_KEY], "ENG");

        let s = Settings {
            linear_api_key: "old".into(),
            bundle_values: flags(&[(SETTING_LINEAR_API_KEY, "new")]),
            ..Default::default()
        };
        assert_eq!(s.bundle_value(SETTING_LINEAR_API_KEY), "new");
        let got = s.migrate_bundle_values();
        assert_eq!(
            got.bundle_values[SETTING_LINEAR_API_KEY], "new",
            "existing BundleValues win"
        );
        assert_eq!(got.bundle_value(SETTING_LINEAR_API_KEY), "new");

        let blank = Settings {
            linear_api_key: " k ".into(),
            bundle_values: flags(&[(SETTING_LINEAR_API_KEY, "  ")]),
            ..Default::default()
        };
        assert_eq!(blank.bundle_value(SETTING_LINEAR_API_KEY), "k");
        assert_eq!(
            blank.migrate_bundle_values().bundle_values[SETTING_LINEAR_API_KEY],
            "k"
        );
        assert_eq!(Settings::default().bundle_value("x"), "");
        assert!(
            Settings::default()
                .migrate_bundle_values()
                .bundle_values
                .is_empty()
        );
    }

    #[test]
    fn skill_and_simplify() {
        let empty = Settings::default();
        assert!(empty.skill_enabled("agent-onboarding"));
        let set = Settings {
            disabled_skills: vec!["agent-onboarding".into(), "frontend-design".into()],
            ..Default::default()
        };
        assert!(!set.skill_enabled("agent-onboarding"));
        assert!(set.skill_enabled("create-skill"));

        assert!(empty.simplify_enabled());
        assert!(
            !Settings {
                simplify: Some(false),
                ..Default::default()
            }
            .simplify_enabled()
        );
        assert!(
            Settings {
                simplify: Some(true),
                ..Default::default()
            }
            .simplify_enabled()
        );
    }

    #[test]
    fn model_for_chat_mode() {
        let set = Settings {
            model_ask: "claude-code/haiku".into(),
            model_plan: "claude-code/opus".into(),
            model_agent: "claude-code/sonnet".into(),
            model_review: "claude-code/fable".into(),
            model_loop: "claude-code/opus".into(),
            flags_ask: flags(&[("effort", "low")]),
            flags_plan: flags(&[("effort", "high")]),
            flags_agent: flags(&[("effort", "medium")]),
            flags_review: flags(&[("effort", "xhigh")]),
            flags_loop: flags(&[("effort", "high")]),
            ..Default::default()
        };
        for (mode, want_model, want_flag) in [
            (CHAT_MODE_ASK, "claude-code/haiku", "low"),
            (CHAT_MODE_PLAN, "claude-code/opus", "high"),
            ("agent", "claude-code/sonnet", "medium"),
            ("", "claude-code/sonnet", "medium"),
            (CHAT_MODE_REVIEW, "claude-code/fable", "xhigh"),
            (CHAT_MODE_LOOP, "claude-code/opus", "high"),
        ] {
            let (model, f) = set.model_for_chat_mode(mode);
            assert_eq!(model, want_model, "{mode:?}");
            assert_eq!(f["effort"], want_flag, "{mode:?}");
        }
    }

    #[test]
    fn child_workspace_model() {
        for (input, want) in [
            ("", CHILD_WORKSPACE_MODEL_PARENT),
            (CHILD_WORKSPACE_MODEL_PARENT, CHILD_WORKSPACE_MODEL_PARENT),
            (CHILD_WORKSPACE_MODEL_MODE, CHILD_WORKSPACE_MODEL_MODE),
            (" mode ", CHILD_WORKSPACE_MODEL_MODE),
            ("junk", CHILD_WORKSPACE_MODEL_PARENT),
        ] {
            let s = Settings {
                child_workspace_model: input.into(),
                ..Default::default()
            };
            assert_eq!(s.effective_child_workspace_model(), want, "{input:?}");
        }
    }

    #[test]
    fn review_falls_back_to_loop() {
        let set = Settings {
            model_ask: "claude-code/haiku".into(),
            model_agent: "claude-code/sonnet".into(),
            model_loop: "claude-code/opus".into(),
            ..Default::default()
        };
        assert_eq!(
            set.model_for_chat_mode(CHAT_MODE_REVIEW).0,
            "claude-code/opus"
        );
    }

    #[test]
    fn loop_role_accessors_alias_modes() {
        let set = Settings {
            model_agent: "cursor/composer-2.5".into(),
            model_review: "claude-code/haiku".into(),
            model_loop: "claude-code/opus".into(),
            flags_agent: flags(&[("fast", "true")]),
            flags_review: flags(&[("effort", "low")]),
            flags_loop: flags(&[("effort", "high")]),
            ..Default::default()
        };
        assert_eq!(set.maestro_model().to_string(), "claude-code/opus");
        assert_eq!(set.coder_model().to_string(), "cursor/composer-2.5");
        assert_eq!(set.reviewer_model().to_string(), "claude-code/haiku");
        assert_eq!(set.maestro_flags()["effort"], "high");
        assert_eq!(set.coder_flags()["fast"], "true");
        assert_eq!(set.reviewer_flags()["effort"], "low");
    }

    #[test]
    fn workspace_title_model_and_uses_provider() {
        let s = Settings::default();
        assert_eq!(
            s.workspace_title_model().to_string(),
            DEFAULT_WORKSPACE_TITLE_MODEL
        );
        assert!(s.uses_provider(PROVIDER_CLAUDE_CODE));
        assert!(!s.uses_provider("cursor"));
        let s = Settings {
            model_workspace_title: "cursor/composer-2".into(),
            ..Default::default()
        };
        assert!(s.uses_provider("cursor"));
        assert!(s.workspace_title_flags().is_empty());
    }

    #[test]
    fn permissions_and_credentials() {
        assert_eq!(Settings::default().permission_mode(), PERMISSION_MODE_ASK);
        let bypass = Settings {
            bypass_permissions: true,
            ..Default::default()
        };
        assert_eq!(bypass.permission_mode(), PERMISSION_MODE_BYPASS);
        assert!(!Settings::default().cursor_configured());
        assert!(
            Settings {
                cursor_api_key: " k ".into(),
                ..Default::default()
            }
            .cursor_configured()
        );
        assert!(
            !Settings {
                codex_api_key: "  ".into(),
                ..Default::default()
            }
            .codex_configured()
        );
        assert!(
            Settings {
                codex_chat_gpt_auth: true,
                ..Default::default()
            }
            .codex_configured()
        );
    }

    #[test]
    fn turn_timeout_minutes() {
        for (input, want) in [
            (0, DEFAULT_AGENT_TURN_TIMEOUT_MINUTES),
            (-1, DEFAULT_AGENT_TURN_TIMEOUT_MINUTES),
            (1, MIN_AGENT_TURN_TIMEOUT_MINUTES),
            (5, 5),
            (240, 240),
            (480, 480),
            (
                MAX_AGENT_TURN_TIMEOUT_MINUTES,
                MAX_AGENT_TURN_TIMEOUT_MINUTES,
            ),
            (
                MAX_AGENT_TURN_TIMEOUT_MINUTES + 1,
                MAX_AGENT_TURN_TIMEOUT_MINUTES,
            ),
        ] {
            assert_eq!(effective_agent_turn_timeout_minutes(input), want, "{input}");
        }
    }

    #[test]
    fn agent_turn_timeout() {
        assert_eq!(
            Settings::default().agent_turn_timeout(),
            Duration::from_secs(4 * 3600)
        );
        let s = Settings {
            agent_turn_timeout_minutes: 90,
            ..Default::default()
        };
        assert_eq!(s.agent_turn_timeout(), Duration::from_secs(90 * 60));
    }

    #[test]
    fn max_loops_agents() {
        assert_eq!(effective_max_loops(0), 0, "zero is unlimited");
        assert_eq!(effective_max_agents(0), 0, "zero is unlimited");
        assert_eq!(effective_max_loops(-1), DEFAULT_MAX_LOOPS);
        assert_eq!(effective_max_agents(-3), DEFAULT_MAX_AGENTS);
        assert_eq!(effective_max_loops(2), 2);
        assert_eq!(effective_max_agents(128), 128, "no upper clamp");
    }

    #[test]
    fn take_and_put_secrets_round_trip() {
        let kv = |k: &str, v: &str| BTreeMap::from([(k.to_owned(), v.to_owned())]);
        let full = Settings {
            cursor_api_key: "cur".into(),
            codex_api_key: "cdx".into(),
            linear_api_key: "lin".into(),
            bundle_values: kv(SETTING_LINEAR_API_KEY, "lin2"),
            compatible_providers: vec![CompatibleProvider {
                id: "groq".into(),
                api_key: "gk".into(),
                extra_headers: kv("Authorization", "Bearer x"),
                ..Default::default()
            }],
            mcp_servers: vec![McpServer {
                id: "acme".into(),
                headers: kv("X-Token", "t"),
                env: kv("API_KEY", "e"),
                ..Default::default()
            }],
            model_ask: "claude-code/opus".into(),
            ..Default::default()
        };
        let mut row = full.clone();
        let secrets = row.take_secrets();
        assert!(!secrets.is_empty());
        let json = serde_json::to_string(&row).unwrap();
        for leaked in ["cur", "cdx", "lin", "gk", "Bearer", "X-Token", "API_KEY"] {
            assert!(!json.contains(&format!("\"{leaked}")), "{leaked} in {json}");
        }
        assert_eq!(row.model_ask, "claude-code/opus");
        row.put_secrets(secrets);
        assert_eq!(row, full);
    }
}
