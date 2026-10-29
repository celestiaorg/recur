import "../src/styles/app.scss";
import "./transport";
import { mount, tick } from "svelte";
import { workspaceId, prId } from "./transport";
import {
  setTheme,
  setEditorTheme,
  openWorkspaceChat,
  openPR,
  observatoryJump,
  setPanelWidthLive,
} from "../src/lib/stores";
import { loadEditorThemes } from "../src/lib/cm-themes";

// Import after the demo transport is installed: no native service can run.
const { default: Preview } = await import("./Preview.svelte");
const { loadInitial } = await import("../src/lib/api");
setTheme("paper");
setPanelWidthLive("rail", 220);
setPanelWidthLive("maestroDock", 390);
setPanelWidthLive("obsSide", 256);
await loadInitial();
await loadEditorThemes();
openWorkspaceChat(workspaceId, "PA");
mount(Preview, { target: document.getElementById("app")! });
await tick();

let lastScene = "";
window.addEventListener("message", async (event) => {
  if (
    event.source !== parent ||
    event.origin !== location.origin ||
    event.data?.type !== "recur-preview"
  )
    return;
  const { app, editor, scene, main, tab, navigate } = event.data;
  if (typeof app !== "string" || typeof editor !== "string") return;
  setTheme(app);
  setEditorTheme(editor);
  const sceneKey = `${scene}:${main}:${tab}`;
  if (navigate || sceneKey !== lastScene) {
    lastScene = sceneKey;
    if (scene === "review") {
      openPR(prId);
    } else {
      openWorkspaceChat(
        workspaceId,
        scene === "loop" ? "MA" : tab === "review" ? "PA2" : "PA",
      );
      await tick();
      if (main === "changes")
        observatoryJump.set({ workspaceId, panel: "review" });
      else observatoryJump.set({ workspaceId, path: "src/main.go", line: 13 });
    }
  }
});
parent.postMessage({ type: "recur-preview-ready" }, location.origin);
