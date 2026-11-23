/** The page's copy, links and download target. */
import { highlightToml } from "./toml";

export const wordmark = "/images/recur-wordmark.png";
export const icon = "/images/recur-icon.png";
export const downloads =
  import.meta.env.VITE_DOWNLOADS_URL ||
  "https://github.com/celestiaorg/recur/releases/latest/download";
export const github = "https://github.com/celestiaorg/recur";
export const x = "https://x.com/recur_build";
export const telegram = "https://t.me/recur_build";
export const linuxBuilds = [
  { arch: "amd64", label: "x64" },
  { arch: "arm64", label: "arm64" },
];
export const linuxFormats = ["deb", "rpm", "pkg.tar.zst", "tar.gz"];
const ua = navigator.userAgent;
const onLinux = /Linux/.test(ua) && !/Android/.test(ua);
const linuxFormat = /Fedora|Red Hat|CentOS|SUSE|Mageia/i.test(ua)
  ? "rpm"
  : "deb";
// Chromium freezes the UA to x86_64, so ask Client Hints for the real arch.
let linuxArch = $state(/aarch64|arm64|armv8/i.test(ua) ? "arm64" : "amd64");
if (onLinux)
  navigator.userAgentData
    ?.getHighEntropyValues(["architecture"])
    .then(({ architecture }) => {
      if (architecture === "arm") linuxArch = "arm64";
    });
export const visitor = {
  get download() {
    return onLinux
      ? `${downloads}/Recur-linux-${linuxArch}.${linuxFormat}`
      : `${downloads}/Recur-macos-universal.dmg`;
  },
};
export const downloadLabel = onLinux
  ? `Download .${linuxFormat} for Linux`
  : "Download for Mac";
export const docs = "/docs/";

export type Screen =
  | "loop"
  | "inbox"
  | "workspace-pr"
  | "pull-request"
  | "changes"
  | "uncommitted"
  | "background"
  | "schedules"
  | "issues"
  | "insights"
  | "workspace"
  | "pull-requests"
  | "inbox-passkey"
  | "inbox-checkout"
  | "inbox-dark-mode"
  | "inbox-recovery"
  | "inbox-session"
  | "inbox-webhook"
  | "inbox-idempotency";
export const screens: { label: string; screen: Screen; caption: string }[] = [
  {
    label: "Autopilot",
    screen: "loop",
    caption:
      "A To a PR workflow fanned out into three child workspaces. Each one merges back as soon as it converges.",
  },
  {
    label: "Inbox",
    screen: "inbox",
    caption:
      "Every workspace in one list, with what it needs beside it. Answer it there, or tell Ask Recur what to do next.",
  },
  {
    label: "Pull request",
    screen: "workspace-pr",
    caption:
      "A pull request opens in its workspace: conversation, commits, diff, and checks, beside the chat that wrote it.",
  },
  {
    label: "PR review",
    screen: "pull-request",
    caption:
      "A pull request opened outside Recur. Review it with an agent, then post the findings as comments.",
  },
  {
    label: "Diff",
    screen: "changes",
    caption:
      "Read the worktree diff against its base and leave inline comments before anything is pushed.",
  },
  {
    label: "Commit",
    screen: "uncommitted",
    caption:
      "Uncommitted diffs are live editors. Check files into the commit, fix a line as you read, and commit.",
  },
  {
    label: "Background",
    screen: "background",
    caption:
      "Dev servers and test runs get a terminal tab that outlives the turn, and agents check back when it’s time.",
  },
  {
    label: "Schedules",
    screen: "schedules",
    caption:
      "Prompts and workflows on a cron, once, at every launch, or whenever a command prints a line. Each run gets a fresh workspace.",
  },
  {
    label: "Issues",
    screen: "issues",
    caption:
      "GitHub and Linear issues assigned to you or open in your projects. Start a workspace from any of them.",
  },
  {
    label: "Insights",
    screen: "insights",
    caption:
      "Where your agents’ hours went, what they lost time to, and how long they waited on you.",
  },
  {
    label: "Workspace",
    screen: "workspace",
    caption:
      "A workspace is a git worktree on its own branch. The chat and the code sit side by side.",
  },
  {
    label: "Pull requests",
    screen: "pull-requests",
    caption:
      "Every open PR across your projects, including ones opened outside Recur. The ones that need you come first.",
  },
];

export const modes = [
  {
    name: "Ask",
    access: "read",
    title: "Questions, answered from the repo.",
    body: "The agent explores the code and answers, or writes a report. Nothing in the worktree changes.",
    example: "Where does the session refresh happen, and what calls it?",
    outcome: "read-only · answers and reports",
  },
  {
    name: "Plan",
    access: "read",
    title: "Investigate before anyone edits.",
    body: "The agent traces the code and writes a plan: the files, the steps, the risks. Hand it to an Agent chat or a workflow.",
    example: "Plan expiring team invitations for this app.",
    outcome: "read-only · a structured plan",
  },
  {
    name: "Agent",
    access: "edit",
    title: "Pair on the change.",
    body: "Reads and edits files in the workspace’s worktree. Steer it mid-turn or queue the next message, or let it spawn a child workspace for a subtask and merge the result back.",
    example: "Add a timeout to the session refresh request.",
    outcome: "read + edit · child workspaces",
  },
  {
    name: "Review",
    access: "read",
    title: "A second pass on the diff.",
    body: "Reviews the worktree diff and files findings with a severity and a location. Hand them back with /fix-findings.",
    example: "Review this change for bugs and unnecessary complexity.",
    outcome: "read-only · findings",
  },
  {
    name: "Autopilot",
    access: "team",
    title: "Where workflows run.",
    body: "Pick a workflow and a policy in the composer, or skip the picking: a workflow slash command such as /review-and-fix in any other chat opens an Autopilot chat beside it. The Maestro reports back here; talk to it to steer the run.",
    example: "Add team invites with links that expire after 48 hours.",
    outcome: "Autopilot · To a PR · Balanced",
  },
];

export const policies = [
  ["Cautious", "any finding", "always", "2, escalate"],
  ["Balanced", "medium+", "if structured", "4, escalate"],
  ["Aggressive", "high+", "never", "6, proceed"],
];
export const loopExample = highlightToml(`# ~/.recur/bundles/Team/bundle.toml
[[role]]
id = "team.roles.api-reviewer"
name = "API reviewer"
code = "AR"
mode = "review"
prompt = """
Review public API changes only. File a finding with
report_finding for each breaking change, missing
version bump, or undocumented error. Breaking is high.
"""

[[workflow]]
id = "team.workflows.api-change"
name = "API change"
conductor = "recur.roles.maestro"
entry = "plan"

[[workflow.phase]]
id = "plan"
uses = "plan"
role = "recur.roles.planner"
prompt = "Plan from the OpenAPI spec. List every endpoint that changes."
gate = "plan_approval"
next = "build"

[[workflow.phase]]
id = "build"
uses = "converge"
next = "test"

[workflow.phase.roles]
implementer = "recur.roles.coder"
reviewer = "team.roles.api-reviewer"

[workflow.phase.prompts]
implement = "Update the OpenAPI spec first, then the handlers."
fix = "/fix-findings"

[[workflow.phase]]
id = "test"
uses = "run"
command = "mise run check"
on_reject = "build"
next = "check"

[[workflow.phase]]
id = "check"
uses = "verify"
role = "recur.roles.qa"
prompt = "Call each changed endpoint and attach the responses."
on_reject = "build"
next = "pr"

[[workflow.phase]]
id = "pr"
uses = "ensure_pr"`);

export const keys: [string[], string][] = [
  [["⌘N"], "New workspace"],
  [["⌘K"], "Go to any workspace or PR"],
  [["⌘P"], "Open a file"],
  [["⌘⇧P"], "Run a bundle command"],
  [["⌘."], "Switch chat mode"],
  [["⌘L"], "Add the selection to chat"],
  [["⌘1", "⌘2", "⌘3"], "Focus chat, editor, terminal"],
  [["⌥⌘↑", "⌥⌘↓"], "Previous / next workspace"],
  [["J", "K"], "Next / previous file in review"],
  [["⌘R"], "Start a project run"],
  [["⌘⇧⌫"], "Stop the agent"],
  [["⌃`"], "Toggle the terminal"],
];
export const loops = [
  ["/review-and-fix", "Review the diff in the workspace, yours or the agent’s, and fix it until it clears the bar."],
  ["/to-a-pr", "Plan, build, review, and open a pull request, all from one sentence."],
  ["/review-to-pr", "Review & fix, then open or update the pull request."],
  ["/security-review", "Review & fix focused on auth, secrets, tokens, and permission checks."],
  ["/write-docs", "Plan, write, and edit docs until the editor runs out of findings."],
];
