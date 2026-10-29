import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { parseArgs } from "node:util";
import {
  readFile,
  writeFile,
  mkdir,
  mkdtemp,
  copyFile,
  rm,
} from "node:fs/promises";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const require = createRequire(
  new URL("../src-web/demo/package.json", import.meta.url),
);
const { chromium, expect } = require("@playwright/test");
const { values } = parseArgs({
  options: {
    screen: { type: "string" },
    theme: { type: "string" },
    output: { type: "string" },
    list: { type: "boolean", default: false },
  },
});
const datasetBytes = await readFile(
  join(root, "src-web/demo/captures/dataset.json"),
);
const presetsBytes = await readFile(
  join(root, "src-web/demo/captures/screens.json"),
);
const dataset = JSON.parse(datasetBytes);
const presets = JSON.parse(presetsBytes);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const safeName = (name) =>
  typeof name === "string" && /^[a-z][a-z0-9-]*$/.test(name);
assert.equal(presets.version, 1);
assert.equal(dataset.version, 1);
assert.ok(
  Number.isFinite(Date.parse(dataset.clock)),
  "Dataset clock must be an ISO date",
);
assert.ok(
  [1, 2].includes(presets.deviceScaleFactor),
  "Use 1x or 2x capture resolution",
);
assert.equal(
  new Set(presets.screens.map((s) => s.id)).size,
  presets.screens.length,
  "Screen IDs must be unique",
);
const sharedFrame = presets.screens[0];
for (const screen of presets.screens) {
  assert.ok(safeName(screen.id), "Screen IDs must be safe filenames");
  assert.deepEqual(
    screen.viewport,
    sharedFrame.viewport,
    "Tour captures must share a window size",
  );
  assert.equal(
    screen.widths.rail,
    sharedFrame.widths.rail,
    "Tour captures must share a sidebar width",
  );
  assert.ok(screen.viewport.width >= 640 && screen.viewport.width <= 2560);
  assert.ok(screen.viewport.height >= 400 && screen.viewport.height <= 1600);
  assert.ok(["workspace", "pr", "inbox", "issues", "schedules"].includes(screen.view));
  assert.ok(["editor", "review"].includes(screen.surface));
  assert.ok(["conv", "files", "commits", "checks"].includes(screen.prTab));
  assert.ok(
    dataset.tabs.some((t) => t.code === screen.chat),
    `Unknown chat in ${screen.id}`,
  );
  assert.ok(screen.file in dataset.files, `Unknown file in ${screen.id}`);
  for (const pane of ["sidebar", "chat", "files", "prChat"])
    assert.equal(typeof screen.panes[pane], "boolean");
  for (const width of Object.values(screen.widths))
    assert.ok(Number.isInteger(width) && width > 0);
  assert.ok(screen.target && screen.ready.selector && screen.ready.text);
}
if (values.list) {
  for (const s of presets.screens)
    console.log(
      `${s.id}: ${s.title} (${s.viewport.width} × ${s.viewport.height})`,
    );
  process.exit(0);
}
const screens = presets.screens.filter(
  (s) => !values.screen || s.id === values.screen,
);
const themes = Object.entries(presets.themes).filter(
  ([name]) => !values.theme || name === values.theme,
);
assert.ok(screens.length, `Unknown screen: ${values.screen}`);
assert.ok(themes.length, `Unknown theme: ${values.theme}`);
const output = resolve(
  values.output ?? join(root, "src-web/public/screenshots"),
);
const stage = await mkdtemp(join(tmpdir(), "recur-captures-"));
let server, browser;
try {
  const { preview } = await import(require.resolve("vite"));
  server = await preview({
    configFile: false,
    root: join(root, "src-web"),
    base: "/demo/",
    build: { outDir: "public/demo" },
    preview: { host: "127.0.0.1", port: 5185, strictPort: true },
  });
  browser = await chromium.launch({
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
  });
  const source = {
    commit: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: root,
      encoding: "utf8",
    }).trim(),
    appDirty: !!execFileSync(
      "git",
      ["diff", "HEAD", "--", "src-web/demo/src", "src-web/demo/bindings"],
      { cwd: root, encoding: "utf8" },
    ).trim(),
    datasetSha256: sha(datasetBytes),
    presetsSha256: sha(presetsBytes),
    browser: browser.version(),
    platform: process.platform,
    clock: dataset.clock,
  };
  const captures = [];
  for (const screen of screens)
    for (const [themeName, theme] of themes) {
      assert.ok(safeName(themeName));
      const id = `${screen.id}-${themeName}`;
      const context = await browser.newContext({
        viewport: screen.viewport,
        deviceScaleFactor: presets.deviceScaleFactor,
        reducedMotion: "reduce",
        locale: "en-US",
        timezoneId: "UTC",
      });
      try {
        const page = await context.newPage();
        const errors = [],
          unexpectedRequests = [];
        page.on("pageerror", (error) => errors.push(error.message));
        await page.route("**/*", (route) => {
          const url = new URL(route.request().url());
          if (
            url.origin !== "http://127.0.0.1:5185" ||
            /\/wails\/runtime|\/workspace-file/.test(url.pathname)
          ) {
            unexpectedRequests.push(url.href);
            return route.abort();
          }
          return route.continue();
        });
        await page.clock.setFixedTime(new Date(dataset.clock));
        await page.goto(
          `http://127.0.0.1:5185/demo/captures/index.html?screen=${screen.id}&theme=${themeName}`,
        );
        await expect(page.locator("html")).toHaveAttribute(
          "data-capture-ready",
          screen.id,
        );
        await expect(page.locator(screen.ready.selector).first()).toContainText(
          screen.ready.text,
        );
        await expect(page.locator(".view.on .skel:visible")).toHaveCount(0);
        await page.evaluate(async () => {
          await Promise.all([
            document.fonts.load("400 13px Inter"),
            document.fonts.load("500 13px Inter"),
            document.fonts.load('400 13px "JetBrains Mono"'),
          ]);
          await document.fonts.ready;
          await Promise.all(
            [...document.images]
              .filter((img) => img.checkVisibility())
              .map(async (img) => {
                try {
                  await img.decode();
                } catch {
                  throw new Error(`Capture image did not load: ${img.src}`);
                }
              }),
          );
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
        });
        for (const family of ["Inter", "JetBrains Mono"]) {
          assert.ok(
            await page.evaluate(
              (f) =>
                [...document.fonts].some(
                  (font) =>
                    font.family.replaceAll('"', "") === f &&
                    font.status === "loaded",
                ),
              family,
            ),
            `${family} did not load`,
          );
        }
        await expect(page.locator(".rail"))[
          screen.panes.sidebar ? "toBeVisible" : "toBeHidden"
        ]();
        if (screen.panes.sidebar)
          await expect(page.locator(".rail")).toHaveCSS(
            "width",
            `${sharedFrame.widths.rail}px`,
          );
        if (screen.view === "workspace") {
          await expect(page.locator("#v-detail .obs-left"))[
            screen.panes.chat ? "toBeVisible" : "toBeHidden"
          ]();
          await expect(page.locator("#v-detail .obs-side"))[
            screen.panes.files ? "toBeVisible" : "toBeHidden"
          ]();
        } else if (screen.view === "inbox")
          await expect(page.locator("#v-briefing .briefing-ask"))[
            screen.panes.chat ? "toBeVisible" : "toBeHidden"
          ]();
        else if (screen.view === "pr")
          await expect(page.locator("#prd-agent"))[
            screen.panes.prChat ? "toBeVisible" : "toBeHidden"
          ]();
        await expect(page.locator(".toast.error")).toHaveCount(0);
        assert.deepEqual(errors, [], `${id}: browser errors`);
        assert.deepEqual(
          unexpectedRequests,
          [],
          `${id}: unexpected network requests`,
        );
        if (screen.target === ".win") {
          await expect(page.locator(".tb-traffic-clear")).toBeVisible();
          assert.equal(
            await page
              .locator(".tb-traffic-clear")
              .evaluate(
                (el) => getComputedStyle(el, "::before").backgroundColor,
              ),
            "rgb(255, 95, 87)",
          );
        }
        await expect(page.locator(".rail .tproj")).toHaveCount(2);
        if (screen.id === "loop") {
          await expect(page.locator(".rail .tchild")).toHaveCount(3);
          await expect(
            page.locator('.rail [data-workspace-id="demo-tests"] .sd.pulse'),
          ).toBeVisible();
          await expect(
            page.locator("#v-detail .fanout-state.running"),
          ).toContainText("coding");
        }
        if (screen.id === "changes") {
          if (theme.app === "paper") {
            await expect(page.locator(".hline.add .cd").first()).toHaveCSS(
              "color",
              "rgb(26, 28, 32)",
            );
            await expect(page.locator(".hline.del .cd").first()).toHaveCSS(
              "color",
              "rgb(26, 28, 32)",
            );
          }
          await expect(page.locator(screen.ready.selector)).toContainText(
            "config.go",
          );
          await expect(page.locator(screen.ready.selector)).toContainText(
            "session_test.go",
          );
        }
        const target = page.locator(screen.target);
        const box = await target.boundingBox();
        assert.ok(
          box && box.width > 0 && box.height > 0,
          `${id}: empty capture target`,
        );
        assert.equal(
          box.width,
          sharedFrame.viewport.width,
          "Capture must retain the full app width",
        );
        assert.equal(
          box.height,
          sharedFrame.viewport.height,
          "Capture must retain the full app height",
        );
        const bytes = await target.screenshot({
          path: join(stage, `${id}.png`),
          animations: "disabled",
          caret: "hide",
        });
        captures.push({
          id,
          file: `${id}.png`,
          title: screen.title,
          placement: screen.placement,
          theme: themeName,
          settings: {
            ...screen,
            theme,
            deviceScaleFactor: presets.deviceScaleFactor,
          },
          dimensions: {
            width: Math.round(box.width * presets.deviceScaleFactor),
            height: Math.round(box.height * presets.deviceScaleFactor),
          },
          sha256: sha(bytes),
          source,
        });
        console.log(
          `Captured ${id}: ${box.width} × ${box.height} @${presets.deviceScaleFactor}x`,
        );
      } finally {
        await context.close();
      }
    }
  // Publish only after every requested screen passed its readiness checks.
  await mkdir(output, { recursive: true });
  let previous = [];
  try {
    previous = JSON.parse(
      await readFile(join(output, "manifest.json"), "utf8"),
    ).captures;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const merged = [
    ...previous.filter((c) => !captures.some((n) => n.id === c.id)),
    ...captures,
  ];
  const order = presets.screens.flatMap((screen) =>
    Object.keys(presets.themes).map((theme) => `${screen.id}-${theme}`),
  );
  merged.sort((a, b) => {
    const position = (id) =>
      order.includes(id) ? order.indexOf(id) : order.length;
    return position(a.id) - position(b.id) || a.id.localeCompare(b.id);
  });
  for (const capture of captures)
    await copyFile(join(stage, capture.file), join(output, capture.file));
  await writeFile(
    join(output, "manifest.json"),
    JSON.stringify({ version: 1, captures: merged }, null, 2) + "\n",
  );
  const escape = (s) =>
    String(s)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll('"', "&quot;");
  for (const capture of merged) {
    const width = capture.dimensions.width / capture.settings.deviceScaleFactor;
    const height =
      capture.dimensions.height / capture.settings.deviceScaleFactor;
    await writeFile(
      join(output, `${capture.id}.html`),
      `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escape(capture.title)}</title><style>body{margin:16px;background:#141414;color:#e6e4e1;font:14px system-ui}a{color:inherit}img{display:block;width:${width}px;max-width:100%;height:auto;margin:16px auto}header{max-width:${width}px;margin:auto;display:flex;justify-content:space-between;gap:16px}</style><header><a href="index.html">← All captures</a><span>${escape(capture.title)} · ${width} × ${height}</span></header><img src="${escape(capture.file)}" alt="${escape(capture.title)}" width="${width}" height="${height}"></html>`,
    );
  }
  await writeFile(
    join(output, "index.html"),
    `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Recur product captures</title><style>body{margin:32px;background:#141414;color:#e6e4e1;font:16px system-ui}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,580px),1fr));gap:28px}figure{margin:0}img{width:100%;height:auto;border:1px solid #3d3d3d;border-radius:8px}figcaption{margin:12px 0;color:#e07a5f}small{color:#aaa}a{color:inherit}pre{overflow:auto;font-size:12px}details{margin:10px 0}</style><h1>Recur product captures</h1><p>Open a capture at full resolution. Each image records its data and layout settings.</p><main>${merged.map((c) => `<figure><a href="${escape(c.id)}.html"><img src="${escape(c.file)}" alt="${escape(c.title)} in ${escape(c.theme)}" width="${c.dimensions.width}" height="${c.dimensions.height}"></a><figcaption>${escape(c.title)} · ${escape(c.theme)}</figcaption><small>${c.dimensions.width / c.settings.deviceScaleFactor} × ${c.dimensions.height / c.settings.deviceScaleFactor} @${c.settings.deviceScaleFactor}x · ${escape(c.placement)}</small><details><summary>Capture settings</summary><pre>${escape(JSON.stringify(c.settings, null, 2))}</pre></details></figure>`).join("")}</main></html>`,
  );
  console.log(`Gallery: ${join(output, "index.html")}`);
} finally {
  await browser?.close();
  server?.httpServer.closeAllConnections();
  if (server) await new Promise((resolve) => server.httpServer.close(resolve));
  await rm(stage, { recursive: true, force: true });
}
