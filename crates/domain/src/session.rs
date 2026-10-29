//! Recur's lossless, provider-neutral transcript store: a Session owns an
//! ordered set of Messages, each Message owns typed ContentBlocks.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::json::null_default;
use crate::serde_util::{is_false, is_zero};

// Context modes. In provider mode the LLM provider owns context assembly and
// Recur only mirrors; in app mode the store IS the context.
pub const CONTEXT_MODE_PROVIDER: &str = "provider";
pub const CONTEXT_MODE_APP: &str = "app";

// Stream status of a message: pending/streaming at turn start, complete once
// its final block is written, error if the turn failed.
pub const STREAM_PENDING: &str = "pending";
pub const STREAM_STREAMING: &str = "streaming";
pub const STREAM_COMPLETE: &str = "complete";
pub const STREAM_ERROR: &str = "error";

// Content-block types. The set is OPEN: a stored type the UI doesn't
// recognize must round-trip and render as a fallback, never error.
pub const BLOCK_TEXT: &str = "text";
pub const BLOCK_THINKING: &str = "thinking";
pub const BLOCK_TOOL_USE: &str = "tool_use";
pub const BLOCK_TOOL_RESULT: &str = "tool_result";
pub const BLOCK_IMAGE: &str = "image";
pub const BLOCK_COMPACT: &str = "compact";
pub const BLOCK_FALLBACK: &str = "fallback";

// CompactPayload.status values (also on ChatItem).
pub const COMPACT_STARTED: &str = "started";
pub const COMPACT_DONE: &str = "done";

// Chat modes: how a chat session's agent may act. The mode is a property of
// the chat session, not the workspace. Enforcement is structural (tool
// scoping), never prompt text.
/// Read-only Q&A over the worktree.
pub const CHAT_MODE_ASK: &str = "ask";
/// Read-only exploration → a structured, approvable plan.
pub const CHAT_MODE_PLAN: &str = "plan";
/// The default coding chat: read + edit.
pub const CHAT_MODE_AGENT: &str = "agent";
/// Read-only review pass; files structured findings.
pub const CHAT_MODE_REVIEW: &str = "review";
/// The autopilot; the chat is the Maestro thread.
pub const CHAT_MODE_LOOP: &str = "loop";

/// Maps empty/unknown modes to "agent", so a stored mode from a newer build
/// degrades to a safe default instead of wedging the chat.
pub fn normalize_chat_mode(m: &str) -> &'static str {
    match m {
        CHAT_MODE_ASK => CHAT_MODE_ASK,
        CHAT_MODE_PLAN => CHAT_MODE_PLAN,
        CHAT_MODE_REVIEW => CHAT_MODE_REVIEW,
        CHAT_MODE_LOOP => CHAT_MODE_LOOP,
        _ => CHAT_MODE_AGENT,
    }
}

/// One agent conversation: a role/kind, a provider, a config, and an ordered
/// transcript. It belongs to a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    /// maestro | coder | reviewer | qa | pair | task | ….
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent_session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fork_message_id: String,
    /// provider | app.
    pub context_mode: String,
    pub provider: String,
    /// e.g. the claude session id for --resume.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider_session_ref: String,
    /// System prompt, tools, default model/effort.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub config_json: String,
    /// The compaction pointer (app mode).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_context_id: String,
    /// idle | streaming | error.
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One compaction watermark: the summary + last compacted message used for
/// fork inject and app-mode request assembly. Re-compaction inserts a new
/// row and repoints session.active_context_id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionContext {
    pub id: String,
    pub session_id: String,
    pub summary: String,
    pub last_compacted_message_id: String,
    pub created_at: i64,
}

/// One turn element (user/assistant/system), stamped with the model and
/// provider that produced it so history is self-describing across mid-session
/// model changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    /// Provenance across forks.
    pub origin_session_id: String,
    /// user | assistant | system.
    pub role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub stream_status: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub started_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub completed_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_creation_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read_tokens: i64,
    /// The wall time of the TURN this message ended. Zero on every message
    /// but the last of a turn.
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: i64,
    /// The model's context limit for the turn that stamped this message.
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
    pub created_at: i64,

    /// In block order. `None` serializes as `null`.
    pub content: Option<Vec<ContentBlock>>,
}

impl Message {
    /// The content blocks, empty when nil.
    pub fn blocks(&self) -> &[ContentBlock] {
        self.content.as_deref().unwrap_or_default()
    }
}

/// One typed part of a message. The payload is normalized JSON text
/// (content_json); Type is an open set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContentBlock {
    pub id: String,
    pub r#type: String,
    /// Raw JSON payload text. Empty serializes as `null`.
    #[serde(rename = "content", with = "crate::json::raw")]
    pub content_json: String,
    /// When the block was mirrored (unix ms); loaded on read.
    #[serde(skip_serializing_if = "is_zero")]
    pub created_at: i64,
}

/// A `text` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextPayload {
    /// Full wire (provider replay).
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    /// UI-only; providers ignore.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub display_text: String,
}

/// A `thinking` block; the signature is preserved for the Anthropic
/// round-trip. EstimatedTokens annotates REDACTED thinking (empty text +
/// signature) so the UI can still render "Thought ~N tokens".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThinkingPayload {
    #[serde(deserialize_with = "null_default")]
    pub thinking: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub signature: String,
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
    pub estimated_tokens: i64,
}

/// A `tool_use` block. It carries BOTH an internal id and the provider's
/// tool-use id (the dual-ID trick that makes cross-provider replay
/// tractable).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolUsePayload {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub provider_tool_use_id: String,
    /// The provider tool_use id of a parent Task when a multiplexed subagent
    /// made this call. Empty for top-level.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub parent_tool_use_id: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Raw JSON input. Empty is omitted.
    #[serde(skip_serializing_if = "String::is_empty", with = "crate::json::raw")]
    pub input: String,
}

/// A `tool_result` block. tool_use_id is the INTERNAL id it answers (the
/// one-result-per-call unique index keys on it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolResultPayload {
    #[serde(deserialize_with = "null_default")]
    pub tool_use_id: String,
    #[serde(deserialize_with = "null_default")]
    pub result: String,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "null_default")]
    pub is_error: bool,
    /// The PROVIDER clipped this result for size.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "null_default")]
    pub truncated: bool,
}

/// An `image` block for a user-pasted chat attachment. Path is relative to
/// the managed data root; bytes live on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImagePayload {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub mime: String,
    #[serde(deserialize_with = "null_default")]
    pub path: String,
}

/// The inline "compacted here" chip. The compaction pointer itself is
/// SessionContext; this block is UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactPayload {
    /// started | done.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    /// auto | manual.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub trigger: String,
    /// Provider summary text when present.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub summary: String,
    /// Tokens before compact, when reported.
    #[serde(
        rename = "preTokens",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    pub pre_tokens: i64,
}

/// A `fallback` block: a model declined and another continued the turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FallbackPayload {
    /// Requested / declining model id.
    #[serde(deserialize_with = "null_default")]
    pub from: String,
    /// Model that continued.
    #[serde(deserialize_with = "null_default")]
    pub to: String,
}

/// An image the user pasted into agent chat (send path). Data is raw base64
/// (no data: URL prefix).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatAttachment {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub mime: String,
    pub data: String,
}

/// A UI/transcript pointer to a persisted chat attachment (no bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatImageRef {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mime: String,
    /// /workspaces/<ws>/attachments/<file>.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

fn block<T: Serialize>(typ: &str, payload: &T) -> ContentBlock {
    ContentBlock {
        r#type: typ.to_owned(),
        // The payloads are plain structs; marshal cannot fail in practice.
        content_json: serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned()),
        ..Default::default()
    }
}

impl ContentBlock {
    /// A normalized text block.
    pub fn text(text: &str) -> Self {
        Self::text_display(text, "")
    }

    /// A text block with optional UI-only display_text. Providers and session
    /// rebuild read Text; the chat flatten uses DisplayText when set.
    pub fn text_display(text: &str, display: &str) -> Self {
        block(
            BLOCK_TEXT,
            &TextPayload {
                text: text.to_owned(),
                display_text: display.to_owned(),
            },
        )
    }

    /// A thinking block. tokens is the running estimate for redacted thinking
    /// (0 when the text itself is present).
    pub fn thinking(thinking: &str, signature: &str, tokens: i64) -> Self {
        block(
            BLOCK_THINKING,
            &ThinkingPayload {
                thinking: thinking.to_owned(),
                signature: signature.to_owned(),
                estimated_tokens: tokens,
            },
        )
    }

    /// A tool_use block. `input` is raw JSON text.
    pub fn tool_use(internal_id: &str, provider_id: &str, name: &str, input: &str) -> Self {
        Self::tool_use_nested(internal_id, provider_id, "", name, input)
    }

    /// [`ContentBlock::tool_use`] with an optional parent Task provider id.
    pub fn tool_use_nested(
        internal_id: &str,
        provider_id: &str,
        parent_provider_id: &str,
        name: &str,
        input: &str,
    ) -> Self {
        block(
            BLOCK_TOOL_USE,
            &ToolUsePayload {
                id: internal_id.to_owned(),
                provider_tool_use_id: provider_id.to_owned(),
                parent_tool_use_id: parent_provider_id.to_owned(),
                name: name.to_owned(),
                input: input.to_owned(),
            },
        )
    }

    /// A tool_result block answering the internal tool_use id.
    pub fn tool_result(internal_id: &str, result: &str, is_error: bool) -> Self {
        Self::truncated_tool_result(internal_id, result, is_error, false)
    }

    /// [`ContentBlock::tool_result`] for a result the provider clipped.
    pub fn truncated_tool_result(
        internal_id: &str,
        result: &str,
        is_error: bool,
        truncated: bool,
    ) -> Self {
        block(
            BLOCK_TOOL_RESULT,
            &ToolResultPayload {
                tool_use_id: internal_id.to_owned(),
                result: result.to_owned(),
                is_error,
                truncated,
            },
        )
    }

    /// An image block for a chat attachment.
    pub fn image(id: &str, name: &str, mime: &str, path: &str) -> Self {
        block(
            BLOCK_IMAGE,
            &ImagePayload {
                id: id.to_owned(),
                name: name.to_owned(),
                mime: mime.to_owned(),
                path: path.to_owned(),
            },
        )
    }

    /// A compact block.
    pub fn compact(status: &str, trigger: &str, summary: &str, pre_tokens: i64) -> Self {
        block(
            BLOCK_COMPACT,
            &CompactPayload {
                status: status.to_owned(),
                trigger: trigger.to_owned(),
                summary: summary.to_owned(),
                pre_tokens,
            },
        )
    }

    /// A model-fallback block.
    pub fn fallback(from: &str, to: &str) -> Self {
        block(
            BLOCK_FALLBACK,
            &FallbackPayload {
                from: from.to_owned(),
                to: to.to_owned(),
            },
        )
    }

    /// Stores an unknown/opaque block type verbatim so it round-trips.
    pub fn raw(typ: &str, payload: &str) -> Self {
        Self {
            r#type: typ.to_owned(),
            content_json: payload.to_owned(),
            ..Default::default()
        }
    }

    fn decode<T: DeserializeOwned + Default>(&self, typ: &str) -> Option<T> {
        if self.r#type != typ {
            return None;
        }
        decode_go(&self.content_json)
    }

    /// The text payload; `None` for other types or an undecodable payload.
    pub fn as_text(&self) -> Option<TextPayload> {
        self.decode(BLOCK_TEXT)
    }

    pub fn as_thinking(&self) -> Option<ThinkingPayload> {
        self.decode(BLOCK_THINKING)
    }

    pub fn as_tool_use(&self) -> Option<ToolUsePayload> {
        self.decode(BLOCK_TOOL_USE)
    }

    pub fn as_tool_result(&self) -> Option<ToolResultPayload> {
        self.decode(BLOCK_TOOL_RESULT)
    }

    pub fn as_image(&self) -> Option<ImagePayload> {
        self.decode(BLOCK_IMAGE)
    }

    pub fn as_compact(&self) -> Option<CompactPayload> {
        self.decode(BLOCK_COMPACT)
    }

    pub fn as_fallback(&self) -> Option<FallbackPayload> {
        self.decode(BLOCK_FALLBACK)
    }

    /// The provider tool-use id of the Agent/Task call this block belongs
    /// to. Empty means the block is from the parent agent.
    pub fn parent_tool_use_id(&self) -> String {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Head {
            #[serde(deserialize_with = "null_default")]
            parent_tool_use_id: String,
        }
        decode_go::<Head>(&self.content_json)
            .map(|h| h.parent_tool_use_id.trim().to_owned())
            .unwrap_or_default()
    }

    /// Stamps parent_tool_use_id onto the payload so nested subagent output
    /// can be reconstructed without a schema migration. The payload
    /// is re-encoded with sorted keys. Blank parents and payloads that are
    /// not JSON objects are left unchanged.
    pub fn with_parent(mut self, parent: &str) -> Self {
        let parent = parent.trim();
        if parent.is_empty() || self.content_json.is_empty() {
            return self;
        }
        let Ok(mut m) = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(&self.content_json)
        else {
            return self;
        };
        let Ok(p) = serde_json::to_string(parent).and_then(RawValue::from_string) else {
            return self;
        };
        m.insert("parent_tool_use_id".to_owned(), p);
        if let Ok(raw) = serde_json::to_string(&m) {
            self.content_json = raw;
        }
        self
    }
}

/// Decodes a struct: a JSON `null` document decodes to the zero value instead
/// of failing.
fn decode_go<T: DeserializeOwned + Default>(raw: &str) -> Option<T> {
    if raw.trim() == "null" {
        return Some(T::default());
    }
    serde_json::from_str(raw).ok()
}

/// One interactive coding-agent chat tab in a workspace Agent panel (codes
/// PA, PA2, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceChatTab {
    pub code: String,
    pub title: String,
    pub updated_at: i64,
    /// ask/plan/agent/review/loop; empty means never persisted (the dock
    /// then defaults to agent).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// Finished a turn the human has not viewed yet.
    #[serde(skip_serializing_if = "is_false")]
    pub unseen: bool,
    /// This tab's `[[loop]]` + `[[policy]]` (one chat = one loop).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub loop_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    /// This tab has engaged its own run.
    #[serde(skip_serializing_if = "is_false")]
    pub loop_started: bool,
    /// A shared slot for fresh_session agent flagships so /commit and
    /// /open-pr reuse one tab instead of the user's working chat.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt_group: String,
}

/// The rollup for unseen chat tabs + inbox unread: sidebar badges and the
/// dock icon badge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AttentionCounts {
    pub unseen: i64,
    pub inbox_unread: i64,
    /// unseen + non-chat inbox unread + app unseen.
    pub dock: i64,
    /// `None` serializes as `null`.
    pub by_workspace: Option<BTreeMap<String, i64>>,
    /// `None` serializes as `null`.
    pub by_project: Option<BTreeMap<String, i64>>,
    /// 1 when the Ask Recur session finished a turn the human has not viewed.
    #[serde(skip_serializing_if = "is_zero")]
    pub app_unseen: i64,
}

/// One agent session summarized for the Plan panel and the Loop chat
/// cockpit: identity, lifecycle, and the flattened transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// `None` serializes as `null`.
    pub items: Option<Vec<ChatItem>>,
}

/// One rendered line of an agent chat transcript: a flattened, UI-ready view
/// of the session's messages/content blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatItem {
    /// user | assistant.
    pub role: String,
    /// text | tool | thinking | turn | compact | fallback.
    pub kind: String,
    /// kind=text | thinking | compact summary | fallback to-label.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// kind=tool: tool name (Bash, Read, …).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// kind=tool: short human summary of the input.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_input: String,
    /// kind=thinking: redacted estimate; kind=compact: pre-compact tokens.
    #[serde(skip_serializing_if = "is_zero")]
    pub tokens: i64,
    /// The agent's own one-line label for a tool call. kind=compact: trigger
    /// (auto|manual). kind=fallback: from-model label.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The tool call's full input JSON for the expanded detail card.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input: String,
    /// The tool call's result text (truncated).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub result: String,
    #[serde(skip_serializing_if = "is_false")]
    pub result_error: bool,
    /// The PROVIDER clipped the result for size.
    #[serde(skip_serializing_if = "is_false")]
    pub result_truncated: bool,
    /// The call has not reported a result yet (still running). An empty
    /// Result is not the same thing.
    #[serde(skip_serializing_if = "is_false")]
    pub result_pending: bool,
    /// Set on kind="turn": how long the provider says the turn took.
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: i64,
    /// The durable message that closed this turn (kind="turn"). Fork points
    /// at this id, never a timestamp.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    /// The turn's start→end snapshot changed files ("diff this turn" and
    /// "reset to here" are available).
    #[serde(skip_serializing_if = "is_false")]
    pub checkpoint: bool,
    /// Context usage on kind="turn" for the chat context meter.
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_creation_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
    /// Images pasted into a user message. Text may contain
    /// ⟦attachment:<id>⟧ markers where they were pasted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ChatImageRef>,
    /// When this row happened (unix ms).
    #[serde(skip_serializing_if = "is_zero")]
    pub ts: i64,
    /// The provider tool-use id (toolu_…) for kind=tool.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_id: String,
    /// Nested subagent output under this row (the parent Task).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ChatItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_match_go_payloads() {
        assert_eq!(ContentBlock::text("a<b").content_json, r#"{"text":"a<b"}"#);
        assert_eq!(
            ContentBlock::text_display("wire", "ui").content_json,
            r#"{"text":"wire","display_text":"ui"}"#
        );
        assert_eq!(
            ContentBlock::thinking("", "sig", 80).content_json,
            r#"{"thinking":"","signature":"sig","estimated_tokens":80}"#
        );
        let tu = ContentBlock::tool_use("tu-1", "toolu_1", "Bash", r#"{ "command": "ls" }"#);
        assert_eq!(tu.r#type, BLOCK_TOOL_USE);
        assert_eq!(
            tu.content_json,
            r#"{"id":"tu-1","provider_tool_use_id":"toolu_1","name":"Bash","input":{ "command": "ls" }}"#
        );
        assert_eq!(
            ContentBlock::tool_use("tu-1", "", "Read", "").content_json,
            r#"{"id":"tu-1","name":"Read"}"#
        );
        assert_eq!(
            ContentBlock::truncated_tool_result("tu-1", "x", true, true).content_json,
            r#"{"tool_use_id":"tu-1","result":"x","is_error":true,"truncated":true}"#
        );
        assert_eq!(
            ContentBlock::tool_result("tu-1", "", false).content_json,
            r#"{"tool_use_id":"tu-1","result":""}"#
        );
        assert_eq!(
            ContentBlock::image("i", "", "image/png", "p").content_json,
            r#"{"id":"i","mime":"image/png","path":"p"}"#
        );
        assert_eq!(
            ContentBlock::compact(COMPACT_DONE, "auto", "s", 12).content_json,
            r#"{"status":"done","trigger":"auto","summary":"s","preTokens":12}"#
        );
        assert_eq!(
            ContentBlock::fallback("a", "b").content_json,
            r#"{"from":"a","to":"b"}"#
        );
        let raw = ContentBlock::raw("evidence", r#"{"path":"x.png"}"#);
        assert_eq!(
            (raw.r#type.as_str(), raw.content_json.as_str()),
            ("evidence", r#"{"path":"x.png"}"#)
        );
    }

    #[test]
    fn accessors_check_type_and_decode() {
        let t = ContentBlock::text_display("wire", "ui");
        assert_eq!(
            t.as_text(),
            Some(TextPayload {
                text: "wire".into(),
                display_text: "ui".into()
            })
        );
        assert_eq!(t.as_thinking(), None);
        let tu = ContentBlock::tool_use_nested("tu", "p", "parent", "Bash", r#"{"a":1}"#);
        let p = tu.as_tool_use().unwrap();
        assert_eq!(
            (p.parent_tool_use_id.as_str(), p.input.as_str()),
            ("parent", r#"{"a":1}"#)
        );
        assert!(
            ContentBlock::tool_result("x", "r", true)
                .as_tool_result()
                .unwrap()
                .is_error
        );
        assert_eq!(
            ContentBlock::image("i", "n", "m", "p")
                .as_image()
                .unwrap()
                .name,
            "n"
        );
        assert_eq!(
            ContentBlock::compact("started", "", "", 0)
                .as_compact()
                .unwrap()
                .status,
            "started"
        );
        assert_eq!(
            ContentBlock::fallback("a", "b").as_fallback().unwrap().to,
            "b"
        );
        assert_eq!(
            ContentBlock::thinking("t", "", 0)
                .as_thinking()
                .unwrap()
                .thinking,
            "t"
        );

        assert_eq!(
            ContentBlock::raw(BLOCK_TEXT, "null").as_text(),
            Some(TextPayload::default())
        );
        assert_eq!(
            ContentBlock::raw(BLOCK_TEXT, r#"{"text":null}"#).as_text(),
            Some(TextPayload::default())
        );
        assert_eq!(ContentBlock::raw(BLOCK_TEXT, "").as_text(), None);
        assert_eq!(ContentBlock::raw(BLOCK_TEXT, "[1]").as_text(), None);
        assert_eq!(
            ContentBlock::raw(BLOCK_TEXT, r#"{"text":5}"#).as_text(),
            None
        );
    }

    #[test]
    fn parent_tool_use_id_and_with_parent() {
        let b = ContentBlock::text("hi");
        assert_eq!(b.parent_tool_use_id(), "");
        let nested = b.clone().with_parent("  toolu_9 ");
        assert_eq!(
            nested.content_json,
            r#"{"parent_tool_use_id":"toolu_9","text":"hi"}"#
        );
        assert_eq!(nested.parent_tool_use_id(), "toolu_9");
        assert_eq!(b.clone().with_parent("  "), b);
        assert_eq!(ContentBlock::raw("x", "").with_parent("p").content_json, "");
        assert_eq!(
            ContentBlock::raw("x", "[1]").with_parent("p").content_json,
            "[1]"
        );
        assert_eq!(
            ContentBlock::raw("x", "null").with_parent("p").content_json,
            "null"
        );
        assert_eq!(
            ContentBlock::raw("x", r#"{"parent_tool_use_id": " a "}"#).parent_tool_use_id(),
            "a"
        );
        assert_eq!(
            ContentBlock::raw("x", r#"{"parent_tool_use_id": 3}"#).parent_tool_use_id(),
            ""
        );
        assert_eq!(
            ContentBlock::raw("x", r#"{"z": { "k" : 1 }, "a": "<"}"#)
                .with_parent("p")
                .content_json,
            r#"{"a":"<","parent_tool_use_id":"p","z":{ "k" : 1 }}"#
        );
    }

    #[test]
    fn normalize_chat_mode_cases() {
        for m in [
            CHAT_MODE_ASK,
            CHAT_MODE_PLAN,
            CHAT_MODE_AGENT,
            CHAT_MODE_REVIEW,
            CHAT_MODE_LOOP,
        ] {
            assert_eq!(normalize_chat_mode(m), m);
        }
        assert_eq!(normalize_chat_mode(""), CHAT_MODE_AGENT);
        assert_eq!(normalize_chat_mode("Ask"), CHAT_MODE_AGENT);
    }

    #[test]
    fn message_json_shape() {
        let m = Message {
            id: "m1".into(),
            role: "user".into(),
            content: Some(vec![ContentBlock::text("hi")]),
            ..Default::default()
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "id": "m1", "originSessionId": "", "role": "user", "streamStatus": "",
                "createdAt": 0,
                "content": [{"id": "", "type": "text", "content": {"text": "hi"}}]
            })
        );
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back.blocks()[0].as_text().unwrap().text, "hi");
        assert!(Message::default().blocks().is_empty());
    }
}
