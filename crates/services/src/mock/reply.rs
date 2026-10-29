//! Decoding a resolver's JSON into a method's return type: `null` lists and
//! maps become `[]` / `{}`, an optional keeps `null`, a `null` struct fails
//! the call, and a `null` scalar is its zero value.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Error, Result};

pub(crate) trait FromReply: Sized {
    fn from_reply(method: &str, v: Value) -> Result<Self>;
}

fn decode<T: DeserializeOwned>(method: &str, v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::new(format!("{method}: {e}")))
}

impl FromReply for () {
    fn from_reply(_: &str, _: Value) -> Result<Self> {
        Ok(())
    }
}

impl<T: DeserializeOwned> FromReply for Vec<T> {
    fn from_reply(method: &str, v: Value) -> Result<Self> {
        if v.is_null() {
            return Ok(Vec::new());
        }
        decode(method, v)
    }
}

impl<V: DeserializeOwned> FromReply for BTreeMap<String, V> {
    fn from_reply(method: &str, v: Value) -> Result<Self> {
        if v.is_null() {
            return Ok(BTreeMap::new());
        }
        decode(method, v)
    }
}

impl<T: DeserializeOwned> FromReply for Option<T> {
    fn from_reply(method: &str, v: Value) -> Result<Self> {
        if v.is_null() {
            return Ok(None);
        }
        decode(method, v).map(Some)
    }
}

impl FromReply for Value {
    fn from_reply(_: &str, v: Value) -> Result<Self> {
        Ok(v)
    }
}

impl FromReply for String {
    fn from_reply(_: &str, v: Value) -> Result<Self> {
        Ok(match v {
            Value::Null => String::new(),
            Value::String(s) => s,
            other => super::js::string(&other),
        })
    }
}

impl FromReply for bool {
    fn from_reply(_: &str, v: Value) -> Result<Self> {
        Ok(super::js::truthy(Some(&v)))
    }
}

impl FromReply for i64 {
    fn from_reply(_: &str, v: Value) -> Result<Self> {
        let n = super::js::number(Some(&v));
        Ok(if n.is_finite() { n.trunc() as i64 } else { 0 })
    }
}

macro_rules! struct_reply {
    ($($t:ty),* $(,)?) => {$(
        impl FromReply for $t {
            fn from_reply(method: &str, v: Value) -> Result<Self> {
                if v.is_null() {
                    return Err(Error::new(format!("{method} returned null")));
                }
                decode(method, v)
            }
        }
    )*};
}

use crate::api::{
    AppChatSelection, ChatGPTLogin, ChatGPTStatus, ClaudeStatus, CodexLogin, CodexStatus,
    CompatibleTestResult, CursorStatus, CursorUsageConnect, EnsureResult, IssuesView,
    PaletteSearchDto, PromptCompleteResultDto, PromptResultDto, RunResultDto, ShellState, Snapshot,
    WorkspaceChatSelection,
};
use recur_domain::{
    Agent, Artifact, AttentionCounts, BriefingSnapshot, Commit, CopyFilesPreview, Escalation,
    FileContent, FileDiff, FileDiffResult, FileSearchResult, Finding, OpenPathResult, Policy, Pr,
    PrComment, Project, ProjectGroup, ReplaceResult, Resource, ScheduledPrompt, SearchResult,
    Settings, UpdateWorkspaceResult, Workspace, WorkspaceChatTab, WorkspaceLandState,
    WorktreeCommitResult,
};

struct_reply!(
    Agent,
    AppChatSelection,
    Artifact,
    AttentionCounts,
    BriefingSnapshot,
    ChatGPTLogin,
    ChatGPTStatus,
    ClaudeStatus,
    CodexLogin,
    CodexStatus,
    Commit,
    CompatibleTestResult,
    CopyFilesPreview,
    CursorStatus,
    CursorUsageConnect,
    EnsureResult,
    Escalation,
    FileContent,
    FileDiff,
    FileDiffResult,
    FileSearchResult,
    Finding,
    IssuesView,
    OpenPathResult,
    PaletteSearchDto,
    Policy,
    Pr,
    PrComment,
    Project,
    ProjectGroup,
    PromptCompleteResultDto,
    PromptResultDto,
    ReplaceResult,
    Resource,
    RunResultDto,
    ScheduledPrompt,
    SearchResult,
    Settings,
    ShellState,
    Snapshot,
    recur_term::Info,
    UpdateWorkspaceResult,
    Workspace,
    WorkspaceChatSelection,
    WorkspaceChatTab,
    WorkspaceLandState,
    WorktreeCommitResult,
);
