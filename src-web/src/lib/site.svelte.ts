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
    label: "Inbox",
    screen: "inbox",
    caption:
      "Every workspace in one list, with what it needs beside it. Answer it there, or tell Ask Recur what to do next.",
  },
  {
    label: "Autopilot",
    screen: "loop",
    caption:
      "A To a PR workflow fanned out into three child workspaces. Each one merges back as soon as it converges.",
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

export const loopExample = highlightToml(`# ~/.recur/bundles/Team/bundle.toml
[[role]]
id = "team.roles.api-reviewer"
name = "API reviewer"
code = "AR"
mode = "review"
prompt = "File a finding for each breaking change."

[[workflow]]
id = "team.workflows.api-change"
name = "API change"
conductor = "recur.roles.maestro"
entry = "build"

[[workflow.phase]]
id = "build"
uses = "converge"
next = "check"

[workflow.phase.roles]
implementer = "recur.roles.coder"
reviewer = "team.roles.api-reviewer"

[[workflow.phase]]
id = "check"
uses = "run"
command = "mise run check"
on_reject = "build"
next = "pr"

[[workflow.phase]]
id = "pr"
uses = "ensure_pr"`);
