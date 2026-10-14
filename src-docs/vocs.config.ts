import { defineConfig } from "vocs/config";

const baseUrl = process.env.DOCS_URL ?? "https://recur.build/docs";
const basePath = new URL(baseUrl).pathname;

export default defineConfig({
  title: "Recur Docs",
  description:
    "User docs for Recur: AI coding workspaces, providers, reviews, Loops, and schedules.",
  baseUrl,
  basePath,
  renderStrategy: "full-static",
  accentColor: "#e07a5f",
  colorScheme: "dark",
  iconUrl: `${basePath}/recur-icon.png`,
  logoUrl: `${basePath}/recur-icon.png`,
  head: {
    meta: {
      themeColor: "#141414",
    },
  },
  codeHighlight: {
    themes: {
      light: "github-light",
      dark: "github-dark",
    },
  },
  topNav: [
    { text: "Product", link: "https://recur.build" },
    { text: "Download", link: "https://recur.build/#download" },
  ],
  sidebar: [
    {
      text: "Start",
      items: [
        { text: "Introduction", link: "/" },
        { text: "Install", link: "/getting-started/install" },
        { text: "Your first workspace", link: "/getting-started/first-workspace" },
      ],
    },
    {
      text: "Concepts",
      items: [
        { text: "Workspaces", link: "/concepts/workspaces" },
        { text: "Project config", link: "/concepts/project-toml" },
        { text: "Chat context", link: "/concepts/chat-context" },
        { text: "Chat modes", link: "/concepts/modes" },
        { text: "Loop mode", link: "/concepts/loop" },
        { text: "Review & checkpoints", link: "/concepts/review" },
        { text: "Pull requests", link: "/concepts/pull-requests" },
        { text: "Issues", link: "/concepts/issues" },
        { text: "Inbox", link: "/concepts/inbox" },
        { text: "Schedules", link: "/concepts/schedules" },
      ],
    },
    {
      text: "Bundles",
      items: [
        { text: "Overview", link: "/bundles" },
        { text: "Create a bundle", link: "/bundles/create" },
        { text: "Manifest", link: "/bundles/manifest" },
        { text: "TypeScript macros", link: "/bundles/typescript" },
        { text: "Shebang commands", link: "/bundles/shebang" },
        { text: "Chat prompts", link: "/bundles/prompts" },
        { text: "Output & save hooks", link: "/bundles/output" },
        { text: "Testing & CLI", link: "/bundles/testing" },
      ],
    },
    {
      text: "Skills",
      items: [
        { text: "Agent skills", link: "/reference/skills" },
      ],
    },
    {
      text: "Reference",
      items: [
        { text: "Providers", link: "/reference/agents" },
        { text: "Integrations & MCP", link: "/reference/integrations" },
        { text: "Settings", link: "/reference/settings" },
        { text: "Themes & shortcuts", link: "/reference/themes" },
        { text: "Troubleshooting", link: "/reference/troubleshooting" },
      ],
    },
  ],
});
