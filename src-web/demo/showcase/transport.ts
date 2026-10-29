// The showcase backend: answers every service call from the product dataset.
// simplify: edits last only for this frame. Running agents, git operations and
// terminals require the downloaded app.
import { setServiceHandler } from "../src/lib/api";
// @ts-expect-error Supplied by the showcase Vite build.
import fixtures from "virtual:showcase-fixtures";
import type rawDataset from "./dataset.json";

const dataset = fixtures.dataset as typeof rawDataset;
const themes = fixtures.themes as { id: string }[];
const themeFiles = fixtures.themeFiles as Record<string, string>;

export const { workspaceId, prId } = dataset;
const { files, diffs, transcript } = dataset;
const gitStatus = Object.fromEntries(
  diffs.map((diff) => [diff.path, diff.status]),
);

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

// Inbox buckets per workspace, as the engine's briefing reports them.
function briefing() {
  const generatedAt = Date.parse(dataset.clock);
  const workspaces = [];
  for (const w of dataset.workspaces) {
    if (w.isMain) continue;
    const bucket =
      w.group === "needs" || w.state === "escalated"
        ? "needs"
        : w.group === "active"
          ? "working"
          : w.group === "ready"
            ? "pickup"
            : "";
    if (!bucket) continue;
    const reason =
      bucket === "needs" ? "question" : bucket === "working" ? "active_agent" : "unfinished_plan";
    workspaces.push({
      workspaceId: w.id,
      projectId: w.projectId,
      projectName: w.projectId,
      title: w.short || w.goal,
      branch: w.branch,
      bucket,
      reasons: [reason],
      now: w.last || w.statusLabel || w.phase || "",
      next:
        bucket === "needs"
          ? "Answer the prompt in this workspace"
          : bucket === "working"
            ? "Let the agent finish this turn"
            : "Pick this up where you left off",
      lastActivityAt: generatedAt - 120000,
      lastSummary: w.last || "",
      state: w.state,
      phase: w.phase,
      diffAdd: w.diff.add,
      diffDel: w.diff.del,
      planDone: 0,
      planTotal: 0,
      primary: { label: "Open", target: `workspace:${w.id}`, kind: "open" },
    });
  }
  return { generatedAt, workspaces };
}

const effort = {
  id: "effort",
  label: "Effort",
  options: [
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High", default: true },
    { value: "xhigh", label: "Extra High" },
    { value: "max", label: "Max" },
  ],
};
const context = {
  id: "context",
  label: "Context",
  options: [
    { value: "200k", label: "200K", default: true },
    { value: "1m", label: "1M" },
  ],
};
const modelCatalog = [
  {
    provider: "claude-code",
    label: "Claude Code",
    models: [
      ["claude-fable-5-1", "Fable 5.1"],
      ["claude-fable-5", "Fable 5"],
      ["claude-opus-5-5", "Opus 5.5"],
      ["claude-opus-5", "Opus 5"],
      ["claude-opus-4-6", "Opus 4.6", true],
      ["claude-sonnet-5-5", "Sonnet 5.5"],
      ["claude-sonnet-5", "Sonnet 5"],
      ["claude-sonnet-4-6", "Sonnet 4.6", true],
      ["claude-haiku-4-5-20251001", "Haiku 4.5"],
    ].map(([id, label, both]) => ({
      ref: `claude-code/${id}`,
      label,
      flags: both ? [effort, context] : [effort],
    })),
  },
];

const handlers: Record<string, (args: any[]) => unknown> = {
  ListProjects: () => dataset.projects,
  ListWorkspaces: () => [...dataset.workspaces].reverse(),
  ListPRs: () => dataset.pullRequests,
  GetPR: (a) => dataset.pullRequests.find((pr) => pr.id === a[0]) ?? null,
  GetInbox: () => dataset.inbox,
  GetSettings: () => dataset.settings,
  AttentionCounts: () => ({ byWorkspace: {}, byProject: {} }),
  GetBriefing: briefing,
  ListModelCatalog: () => modelCatalog,
  ListThemes: () => themes,
  ReadTheme: (a) => themeFiles[a[0]],
  GetEditorSession: () => ({ openFiles: ["src/main.go"], selectedPath: "src/main.go" }),
  ListTree: (a) => listTree(a[2]),
  ListTrees: (a) =>
    Object.fromEntries((a[2] ?? []).map((p: string) => [p, listTree(p)])),
  ReadFile: (a) => {
    if (!(a[2] in files))
      throw new Error("This file is not included in the demo.");
    const content = files[a[2] as keyof typeof files];
    return { path: a[2], content, size: content.length, writable: true };
  },
  GitStatus: () => gitStatus,
  WorkingStatus: () => gitStatus,
  FileAtBase: (a) =>
    (dataset.baseFiles as Record<string, string>)[a[2]] ??
    files[a[2] as keyof typeof files],
  DiffVsBase: () => ({ files: diffs }),
  DiffWorking: () => ({ files: diffs }),
  ListCommits: () => dataset.commits,
  CommitDiff: () => diffs,
  ListFindings: () => dataset.findings,
  ListWorkspaceChats: () => dataset.tabs,
  WorkspaceChatItems: (a) =>
    a[0] === workspaceId && a[1] === "MA" ? dataset.loopTranscript : transcript,
  WorkspaceChatSelection: (a) => ({
    mode: a[1] === "MA" ? "loop" : a[1] === "PA2" ? "review" : "agent",
    model: "claude-code/claude-sonnet-4-6",
    flags: {},
    simplify: true,
  }),
  WorkspaceChatTurnDiff: () => ({
    hash: "9a0e6c1",
    message: "Bound session refresh",
    files: diffs,
  }),
  WorkspaceSessions: () => dataset.sessions,
  WorkspaceFacts: (a) =>
    a[0] === workspaceId
      ? dataset.facts.map((fact) => ({ ...fact, data: JSON.stringify(fact.data) }))
      : [],
};

setServiceHandler(async (name, args) => {
  if (handlers[name]) return structuredClone(handlers[name](args));
  console.warn("Demo service unavailable:", name);
  throw new Error(
    "This is a local demo. Download Recur to run agents and work with your repositories.",
  );
});
