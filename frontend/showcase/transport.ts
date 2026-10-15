import type { Session } from "../src/lib/editor-session";
import dataset from "../captures/dataset.json";
import { setTransport, objectNames } from "@wailsio/runtime";
// @ts-expect-error Supplied by the showcase Vite build, never the desktop build.
import fixtures from "virtual:showcase-fixtures";

const responses = fixtures.responses as Record<string, any>;
const methods = fixtures.methods as Record<number, string>;
export const { workspaceId, prId } = dataset;
const files: Record<string, string> = structuredClone(dataset.files);
const { diffs, transcript, prReview } = dataset;
const gitStatus = Object.fromEntries(
  diffs.map((diff) => [diff.path, diff.status]),
);
let editorSession: Session = {
  openFiles: ["src/main.go"],
  selectedPath: "src/main.go",
  leftKind: "agent",
  rightKind: "files",
  leftOpen: true,
  rightOpen: false,
};

export function setPreviewEditorSession(session: typeof editorSession) {
  editorSession = session;
}

function listTree(path: string) {
  const prefix = path ? `${path}/` : "";
  const entries = new Map<
    string,
    { name: string; path: string; dir: boolean }
  >();
  for (const file of Object.keys(files)) {
    if (!file.startsWith(prefix)) continue;
    const parts = file.slice(prefix.length).split("/");
    const name = parts[0];
    entries.set(name, { name, path: prefix + name, dir: parts.length > 1 });
  }
  return [...entries.values()];
}

// simplify: edits and preferences last only for this frame. Running agents,
// git operations, native dialogs and terminals require the downloaded app.
const local: Record<string, (args: any[]) => unknown> = {
  UpdateProjectRunConfig: (a) => a[0],
  BridgeBenchEnabled: () => false,
  ListLoops: () => [
    {
      id: "recur.loops.pr",
      name: "To a PR",
      description: "Plan, code, review, and open a pull request",
    },
  ],
  ListScheduledPrompts: (a) =>
    dataset.schedules.filter((s) => !a[0] || s.projectId === a[0]),
  AppChatItems: () => dataset.appChat,
  AppChatSelection: () => ({
    mode: "ask",
    model: "claude-code/claude-sonnet-4-6",
    flags: {},
  }),
  GetShellState: () => ({ view: "detail", workspaceId }),
  SetTerminalDockChrome: () => null,
  WorkingCopyState: () => ({
    behindBase: 0,
    aheadBase: 0,
    dirty: false,
    unmerged: false,
    base: "main",
  }),
  GetEditorSession: () => editorSession,
  ListTree: (a) => listTree(a[2]),
  ListTrees: (a) =>
    Object.fromEntries((a[2] ?? []).map((p: string) => [p, listTree(p)])),
  ListFiles: () => Object.keys(files),
  ReadFile: (a) => {
    if (!(a[2] in files))
      throw new Error("This file is not included in the demo.");
    return {
      path: a[2],
      language: a[2].endsWith(".go") ? "go" : "markdown",
      content: files[a[2]],
      size: files[a[2]].length,
      writable: true,
    };
  },
  WriteFile: (a) => {
    if (a[2] in files) files[a[2]] = a[3];
  },
  SearchFiles: (a) => ({
    hits: Object.keys(files)
      .filter((p) => p.includes(a[2] ?? ""))
      .map((path) => ({ path, score: 1 })),
    ready: true,
    building: false,
  }),
  GitStatus: () => gitStatus,
  WorkingStatus: () => gitStatus,
  ListChangedFiles: () => diffs.map((diff) => diff.path),
  FileAtBase: (a) =>
    (dataset.baseFiles as Record<string, string>)[a[2]] ?? files[a[2]],
  FileDiff: (a) =>
    diffs.find((diff) => diff.path === a[2]) ?? {
      path: a[2],
      status: "",
      diff: { add: 0, del: 0 },
      lines: [],
    },
  DiffVsBase: () => ({ files: diffs }),
  DiffWorking: () => ({ files: diffs }),
  ListCommits: () => dataset.commits,
  CommitDiff: () => diffs,
  WorkspaceChatItems: (a) =>
    a[0] === prReview.workspaceId
      ? prReview.transcript
      : a[0] === workspaceId && a[1] === "MA"
        ? dataset.loopTranscript
        : transcript,
  WorkspaceFacts: (a) =>
    a[0] === workspaceId
      ? dataset.facts.map((fact) => ({
          ...fact,
          data: JSON.stringify(fact.data),
        }))
      : [],
  WorkspaceSessions: () => dataset.sessions,
  WorkspaceChatSelection: (a) => ({
    mode:
      a[1] === "MA"
        ? "loop"
        : a[1] === "PA2" || a[0] === prReview.workspaceId
          ? "review"
          : "agent",
    model: "claude-code/claude-sonnet-4-6",
    flags: {},
    simplify: true,
  }),
  ListWorkspaceChats: (a) =>
    a[0] === prReview.workspaceId ? prReview.tabs : dataset.tabs,
  WorkspaceChatTurnDiff: () => ({
    hash: "9a0e6c1",
    message: "Bound session refresh",
    files: diffs,
  }),
  SyncGitHubPRs: () => responses.ListPRs,
  Outline: () => [{ name: "refreshSession", kind: "func", line: 13 }],
  ListFindings: () => dataset.findings,
  ListAssignedIssues: () => dataset.issues,
  GetIssuesView: () => null,
};
const emptyLists = new Set([
  "WorkspaceSessions",
  "WorkspaceFacts",
  "WorkspaceChatModels",
  "ListClosedWorkspaceChats",
  "ListPendingPermissions",
  "ListPendingQuestions",
  "ListTerminalDockChrome",
  "ListTerminals",
  "RestoreWorkspaceTerminals",
  "ListArtifacts",
  "ListPRFindings",
  "ListSkippedBundles",
  "StagedFiles",
]);
const preferences =
  /^(WatchWorkspace|MarkWorkspaceViewed|SetAppChatFocus|MarkAppChatSeen|SetShellState|UpdateEditorSession|ClearEditorSession|SetWorkspaceChatFocus|SetChatUIOccluded|MarkWorkspaceChatSeen|RememberWorkspaceChatSelection|RememberWorkspaceChatSimplify|RememberChatPromptGroup|RememberChatLoopPolicy|SetFileIndexExclude|MarkInboxRead|MarkAllInboxRead|SetIssuesView)$/;

setTransport({
  async call(objectID, _method, _window, request: any) {
    if (objectID !== objectNames.Call) return null;
    const name =
      methods[request?.methodID] ?? request?.methodName?.split(".").pop();
    const args = request?.args ?? [];
    if (local[name]) return structuredClone(local[name](args));
    if (preferences.test(name)) return null;
    if (emptyLists.has(name)) return [];
    const key = `${name}:${args[0]}`;
    if (key in responses) return structuredClone(responses[key]);
    if (name in responses) return structuredClone(responses[name]);
    console.warn("Demo service unavailable:", name);
    throw new Error(
      "This is a local demo. Download Recur to run agents and work with your repositories.",
    );
  },
});
