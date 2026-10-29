import rawDataset from "../captures/dataset.json";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { editorThemes } from "../../src/lib/themes";
import { resolvers, parseTomlArrayTables } from "../e2e/helpers/mock-bridge";

// Build the real frontend for the website. Only the service transport changes.
// Reuse the browser tests' backend-shaped fixtures, never their DOM or styling.
const root = resolve(import.meta.dirname, "..");
// Avatars ship as local files and inline as data URLs: captures reject network.
const dataset: typeof rawDataset = JSON.parse(
  JSON.stringify(rawDataset).replace(
    /"(avatars\/[\w-]+\.jpg)"/g,
    (_, path) =>
      JSON.stringify(
        "data:image/jpeg;base64," +
          readFileSync(resolve(root, "captures", path)).toString("base64"),
      ),
  ),
);
const seed = {
  projects: dataset.projects,
  loops: dataset.workspaces,
  prs: dataset.pullRequests,
  inbox: dataset.inbox,
  settings: dataset.settings,
  policy: dataset.policy,
  resource: dataset.resource,
  loopById: Object.fromEntries(dataset.workspaces.map((w) => [w.id, w])),
  prById: Object.fromEntries(dataset.pullRequests.map((pr) => [pr.id, pr])),
  agentsByLoop: {},
  feedByLoop: {},
  escByLoop: {},
};
const responses: Record<string, unknown> = {};
const methods: Record<number, string> = {};
for (const file of readdirSync(resolve(root, "bindings/recur/internal/app"))) {
  if (!file.endsWith("service.ts")) continue;
  const source = readFileSync(
    resolve(root, "bindings/recur/internal/app", file),
    "utf8",
  );
  for (const match of source.matchAll(
    /export function (\w+)\s*\([^)]*\)[\s\S]*?\$Call\.ByID\((\d+)/g,
  )) {
    methods[Number(match[2])] = match[1];
  }
}
const defaults = [
  "ListProjects",
  "ListProjectGroups",
  "ListWorkspaces",
  "ListPRs",
  "GetInbox",
  "GetPolicy",
  "GetSettings",
  "GetResource",
  "GetProviderUsage",
  "GetBriefing",
  "AttentionCounts",
  "GetInstanceLabel",
  "GetClaudeStatus",
  "GetCursorStatus",
  "ListModelCatalog",
  "ListCommands",
  "ListPromptActions",
  "ListPolicies",
  "ListGrammars",
  "ListSnippets",
  "ListSymbols",
  "ListMonospaceFonts",
  "ListOpenInTargets",
  "ListSkills",
  "ListAllSkills",
  "DetectAgent",
  "ListAppChats",
];
for (const name of defaults)
  responses[name] = resolvers[name]([], seed, resolvers);
// Briefing timestamps belong to the dataset, not the machine doing the build.
const briefing = responses.GetBriefing as {
  generatedAt: number;
  workspaces: { lastActivityAt: number }[];
};
briefing.generatedAt = Date.parse(dataset.clock);
for (const workspace of briefing.workspaces)
  workspace.lastActivityAt = briefing.generatedAt - 120000;
for (const id of [
  ...seed.loops.map((w) => w.id),
  ...seed.prs.map((p) => p.id),
  ...seed.projects.map((p) => p.id),
]) {
  for (const name of [
    "GetWorkspace",
    "GetWorkspaceFeed",
    "ListWorkspaceChats",
    "ListLoopAgents",
    "GetEscalation",
    "GetPR",
    "GetPRFiles",
    "GetPRCommits",
    "GetProject",
    "WorkspaceLandState",
    "FetchWorkingCopyUpstream",
    "ListBranches",
  ]) {
    responses[`${name}:${id}`] = resolvers[name]([id], seed, resolvers);
  }
}
for (const grammar of responses.ListGrammars as { scopeName: string }[]) {
  responses[`ReadGrammar:${grammar.scopeName}`] = resolvers.ReadGrammar(
    [grammar.scopeName],
    seed,
    resolvers,
  );
}
// Include the shipped themes used by the website's visual pickers.
const themes = [];
for (const bundle of ["Recur", "Themes"]) {
  const dir = resolve(root, "../../bundles", bundle);
  for (const theme of parseTomlArrayTables(
    readFileSync(resolve(dir, "bundle.toml"), "utf8"),
    "theme",
  )) {
    if (!editorThemes.some((t) => t.key === theme.id)) continue;
    themes.push({ ...theme, bundleName: bundle, sourceRoot: "default" });
    responses[`ReadTheme:${theme.id}`] = readFileSync(
      resolve(dir, String(theme.path)),
      "utf8",
    );
  }
}
responses.ListThemes = themes;

export default defineConfig({
  root,
  base: "/demo/",
  plugins: [
    svelte(),
    {
      name: "showcase-fixtures",
      resolveId(id) {
        if (id === "virtual:showcase-fixtures") return "\0" + id;
      },
      load(id) {
        if (id === "\0virtual:showcase-fixtures")
          return `export default ${JSON.stringify({ methods, responses })}`;
      },
    },
  ],
  build: {
    target: "esnext",
    outDir: resolve(root, "../public/demo"),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        demo: resolve(root, "showcase/index.html"),
        capture: resolve(root, "captures/index.html"),
      },
    },
  },
});
