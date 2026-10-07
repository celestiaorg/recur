import { defineConfig } from "vocs/config";

export default defineConfig({
  title: "Recur Docs",
  description:
    "User docs for Recur: AI coding workspaces, providers, reviews, Loops, and schedules.",
  baseUrl: "https://docs.recur.build",
  accentColor: "#e07a5f",
  colorScheme: "dark",
  iconUrl: "/recur-icon.png",
  logoUrl: "/recur-icon.png",
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
        { text: "Themes & shortcuts", link: "/reference/themes" },
      ],
    },
  ],
});
