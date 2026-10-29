//! Data the Go app wrote must keep loading: every Go domain struct, as Go
//! encoded it (zero value and a filled value), decodes into the Rust type.
//! `fixtures/go_types.json` is frozen; add a sample by hand for a new type.

use std::collections::BTreeMap;

use recur_domain::*;
use serde::de::DeserializeOwned;

type Fixture = BTreeMap<String, BTreeMap<String, String>>;

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/go_types.json")).expect("fixture parses")
}

fn check<T: DeserializeOwned>(fx: &Fixture, go_name: &str) {
    let samples = fx
        .get(go_name)
        .unwrap_or_else(|| panic!("{go_name} missing from fixture"));
    for which in ["zero", "full"] {
        let go = &samples[which];
        if let Err(e) = serde_json::from_str::<T>(go) {
            panic!("{go_name} {which}: {e}\n{go}");
        }
    }
}

macro_rules! parity {
    ($($go:literal => $ty:ty),+ $(,)?) => {{
        let fx = fixture();
        let names = [$($go),+];
        $( check::<$ty>(&fx, $go); )+
        let missing: Vec<_> = fx.keys().filter(|k| !names.contains(&k.as_str())).collect();
        assert!(missing.is_empty(), "Go types without a decode check: {missing:?}");
    }};
}

#[test]
fn every_go_type_decodes() {
    parity! {
        "Agent" => Agent,
        "AgentSkill" => AgentSkill,
        "AgentStep" => AgentStep,
        "AgentSummary" => AgentSummary,
        "AnalysisRun" => AnalysisRun,
        "Artifact" => Artifact,
        "AttentionCounts" => AttentionCounts,
        "BranchChoice" => BranchChoice,
        "BriefAction" => BriefAction,
        "BriefingCounts" => BriefingCounts,
        "BriefingSnapshot" => BriefingSnapshot,
        "ChatAttachment" => ChatAttachment,
        "ChatHeader" => ChatHeader,
        "ChatImageRef" => ChatImageRef,
        "ChatItem" => ChatItem,
        "Check" => Check,
        "Checkpoint" => Checkpoint,
        "ChildFanOut" => ChildFanOut,
        "Commit" => Commit,
        "CompactPayload" => CompactPayload,
        "CompatibleFlagDim" => CompatibleFlagDim,
        "CompatibleFlagOption" => CompatibleFlagOption,
        "CompatibleModel" => CompatibleModel,
        "CompatibleProvider" => CompatibleProvider,
        "ContentBlock" => ContentBlock,
        "ConvPoint" => ConvPoint,
        "ConversationLine" => ConversationLine,
        "CopyFilesPreview" => CopyFilesPreview,
        "DiffLine" => DiffLine,
        "DiffStat" => DiffStat,
        "DupClone" => DupClone,
        "DupLoc" => DupLoc,
        "Escalation" => Escalation,
        "EscalationOption" => EscalationOption,
        "FallbackPayload" => FallbackPayload,
        "FeedItem" => FeedItem,
        "FileContent" => FileContent,
        "FileDiff" => FileDiff,
        "FileDiffResult" => FileDiffResult,
        "FileSearchHit" => FileSearchHit,
        "FileSearchResult" => FileSearchResult,
        "Finding" => Finding,
        "FindingNote" => FindingNote,
        "FocusNote" => FocusNote,
        "Hotspot" => Hotspot,
        "ImagePayload" => ImagePayload,
        "ImpactItem" => ImpactItem,
        "InboxItem" => InboxItem,
        "IssueRef" => IssueRef,
        "MCPServer" => McpServer,
        "Message" => Message,
        "MetricDelta" => MetricDelta,
        "MetricSnapshot" => MetricSnapshot,
        "ModelRef" => ModelRef,
        "OpenAsk" => OpenAsk,
        "OpenInTarget" => OpenInTarget,
        "OpenPathResult" => OpenPathResult,
        "OutlineSymbol" => OutlineSymbol,
        "PR" => Pr,
        "PRComment" => PrComment,
        "PRParticipant" => PrParticipant,
        "PRReaction" => PrReaction,
        "PRReviewer" => PrReviewer,
        "PRSummary" => PrSummary,
        "PendingLineComment" => PendingLineComment,
        "PermissionDecision" => PermissionDecision,
        "PermissionRequest" => PermissionRequest,
        "Plan" => Plan,
        "PlanAttachment" => PlanAttachment,
        "PlanStep" => PlanStep,
        "Policy" => Policy,
        "PresentWorkspaceResult" => PresentWorkspaceResult,
        "Project" => Project,
        "ProjectGroup" => ProjectGroup,
        "Rail" => Rail,
        "ReplaceResult" => ReplaceResult,
        "Resource" => Resource,
        "RunCommand" => RunCommand,
        "ScheduledPrompt" => ScheduledPrompt,
        "ScheduledPromptRun" => ScheduledPromptRun,
        "SearchHit" => SearchHit,
        "SearchOpts" => SearchOpts,
        "SearchResult" => SearchResult,
        "Session" => Session,
        "SessionContext" => SessionContext,
        "SessionView" => SessionView,
        "Settings" => Settings,
        "StoredEvent" => StoredEvent,
        "SummaryPara" => SummaryPara,
        "SymbolDef" => SymbolDef,
        "SymbolRef" => SymbolRef,
        "TestResult" => TestResult,
        "TextPayload" => TextPayload,
        "ThinkingPayload" => ThinkingPayload,
        "ToolResultPayload" => ToolResultPayload,
        "ToolUsePayload" => ToolUsePayload,
        "TreeEntry" => TreeEntry,
        "UpdateWorkspaceResult" => UpdateWorkspaceResult,
        "UserQuestion" => UserQuestion,
        "UserQuestionAnswer" => UserQuestionAnswer,
        "VerifyStep" => VerifyStep,
        "Workspace" => Workspace,
        "WorkspaceBrief" => WorkspaceBrief,
        "WorkspaceChatTab" => WorkspaceChatTab,
        "WorkspaceContext" => WorkspaceContext,
        "WorkspaceLandState" => WorkspaceLandState,
        "WorktreeCommitResult" => WorktreeCommitResult,
    }
}
