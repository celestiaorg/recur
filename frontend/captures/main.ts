import "../src/styles/app.scss";
import "./fonts.css";
import "./window-chrome.css";
import {
  setPreviewEditorSession,
  workspaceId,
  prId,
} from "../showcase/transport";
import { mount, tick } from "svelte";
import { get } from "svelte/store";
import {
  booted,
  go,
  setTheme,
  setEditorTheme,
  setEditorPrefs,
  setPanelWidthLive,
  railCollapsed,
  prAgentDockOpen,
  inboxChatOpen,
  openWorkspaceChat,
  openPR,
  prDetailTab,
  type PanelKey,
  type EditorPrefs,
} from "../src/lib/stores";
import { ensureTextMateBootstrap } from "../src/lib/tm-bootstrap";
import presets from "./screens.json";

// This entry deliberately mounts the unmodified desktop frontend, not the
// simplified showcase shell. All composition choices are normal app settings.
async function capture() {
  const query = new URLSearchParams(location.search);
  const screen = presets.screens.find((s) => s.id === query.get("screen"));
  const theme =
    presets.themes[query.get("theme") as keyof typeof presets.themes];
  if (!screen || !theme) throw new Error("Unknown capture screen or theme");
  setTheme(theme.app);
  setEditorPrefs(screen.editorPrefs as Partial<EditorPrefs>);
  for (const [key, width] of Object.entries(screen.widths))
    setPanelWidthLive(key as PanelKey, width);
  railCollapsed.set(!screen.panes.sidebar);
  prAgentDockOpen.set(screen.panes.prChat);
  inboxChatOpen.set(false);
  setPreviewEditorSession({
    openFiles: [screen.file],
    selectedPath: screen.file,
    leftKind: "agent",
    rightKind: "files",
    leftOpen: screen.panes.chat,
    rightOpen: screen.panes.files,
    mainSurface: screen.surface as "editor" | "review",
  });

  // Let CodeMirror measure the pinned fonts on its first render, rather than
  // measuring a fallback font and reflowing while the screenshot is taken.
  await Promise.all(
    ["400", "500", "600", "700"].flatMap((weight) => [
      document.fonts.load(`${weight} 13px Inter`),
      document.fonts.load(`${weight} 13px "JetBrains Mono"`),
    ]),
  );
  await document.fonts.ready;

  const { default: App } = await import("../src/App.svelte");
  mount(App, { target: document.getElementById("app")! });
  if (!get(booted))
    await new Promise<void>((resolve) => {
      const stop = booted.subscribe((ready) => {
        if (ready) {
          resolve();
          queueMicrotask(stop);
        }
      });
    });
  await ensureTextMateBootstrap();
  // App's boot restoration and theme catalog finish before applying the preset.
  await tick();
  setEditorTheme(theme.editor);
  if (screen.view === "pr") {
    openPR(prId);
    prDetailTab.set(screen.prTab as "conv" | "files" | "commits" | "checks");
  } else if (screen.view === "inbox") go("briefing");
  else if (screen.view === "schedules") go("schedules");
  else if (screen.view === "issues") go("issues");
  else openWorkspaceChat(workspaceId, screen.chat);
  await tick();
  document.documentElement.dataset.captureReady = screen.id;
}

capture().catch((error) => {
  document.documentElement.dataset.captureError = String(error);
  console.error(error);
});
