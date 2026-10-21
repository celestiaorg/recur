//! Decoding a resolver's JSON into a method's return type, the way the
//! generated Wails bindings' `createFrom` did: `$Create.Array` / `$Create.Map`
//! turn `null` into `[]` / `{}`, `Nullable` keeps `null`, and a struct's
//! `createFrom(null)` throws (the call rejects). A `null` scalar reached the
//! UI as `null`; here it is the Go zero value.

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

macro_rules! scalar_reply {
    ($($t:ty),*) => {$(
        impl FromReply for $t {
            fn from_reply(method: &str, v: Value) -> Result<Self> {
                if v.is_null() {
                    return Ok(Default::default());
                }
                decode(method, v)
            }
        }
    )*};
}

scalar_reply!(String, bool, i64);

macro_rules! struct_reply {
    ($($t:ty),* $(,)?) => {$(
        impl FromReply for $t {
            fn from_reply(method: &str, v: Value) -> Result<Self> {
                if v.is_null() {
                    return Err(Error::new(format!(
                        "{method}: cannot read a {} from null",
                        stringify!($t)
                    )));
                }
                decode(method, v)
            }
        }
    )*};
}

use crate::api::usage::{CursorUsageConnect, Snapshot};
use crate::api::{
    AppChatSelection, ClaudeStatus, CodexStatus, CompatibleTestResult, CursorStatus, EnsureResult,
    IssuesView, PaletteSearchDto, PromptCompleteResultDto, PromptResultDto, RunResultDto,
    ShellState, WorkspaceChatSelection,
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
    ClaudeStatus,
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
