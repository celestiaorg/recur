// Uses src-web's Playwright dependency. Start the src-web dev or
// preview server, then run: node scripts/check-splash.mjs http://localhost:5174
// With no URL, serves the production build itself. Task: mise run test:splash.
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(
  new URL("../src-web/package.json", import.meta.url),
);
const { chromium, expect } = require("@playwright/test");
const server = process.argv[2]
  ? null
  : await (
      await import(require.resolve("vite"))
    ).preview({
      configFile: false,
      root: new URL("../src-web", import.meta.url).pathname,
      preview: { host: "127.0.0.1", port: 5183, strictPort: true },
    });

let browser;
try {
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
  });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "reduce",
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const url = new URL(process.argv[2] ?? "http://127.0.0.1:5183");
  await page.goto(url.href);

  assert.match(await page.locator("h1").innerText(), /until it’s right/);
  assert.equal(
    await page
      .locator(".bn-nav img")
      .evaluate((el) => el.naturalWidth),
    720,
  );

  // The docs link to recur.build/#download.
  await expect(page.locator("#download .bn-button").first()).toHaveAttribute(
    "href",
    /Recur-/,
  );

  for (const [block, id] of [
    [".bn-hero", "bn-hero-downloads"],
    [".bn-close", "bn-close-downloads"],
  ]) {
    await page
      .locator(block)
      .getByRole("button", { name: /Other downloads/ })
      .click();
    const downloads = page.locator(`#${id}`);
    await expect(downloads).toBeVisible();
    await expect(downloads.getByRole("link", { name: ".rpm" })).toHaveCount(2);
    await page.keyboard.press("Escape");
    await expect(downloads).toBeHidden();
  }

  for (const provider of [
    "Claude Code",
    "Cursor",
    "Codex",
    "Antigravity",
    "OpenCode",
  ]) {
    const icon = page
      .locator(".bn-agents li")
      .filter({ hasText: new RegExp(`^${provider}$`) })
      .locator("svg");
    await expect(icon).toBeVisible();
    await expect(icon).toHaveAttribute("aria-hidden", "true");
    await expect(icon.locator("path").first()).toHaveAttribute("fill", "currentColor");
  }

  const loaded = (image) =>
    expect
      .poll(() => image.evaluate((el) => el.complete && el.naturalWidth > 0))
      .toBe(true);
  const viewer = page.getByRole("dialog");

  // The hero is the app in captures: its sidebar and in-window controls navigate.
  const demo = page.locator(".frame");
  const at = (screen) =>
    expect(demo.locator(".layer img").last()).toHaveAttribute(
      "src",
      `/screenshots/${screen}.png`,
    );
  const go = (name) =>
    demo.getByRole("button", { name, exact: true }).click();
  await at("loop");
  for (const [name, screen] of [
    ["Inbox", "inbox"],
    ["Issues", "issues"],
    ["Pull requests", "pull-requests"],
    ["Open #486", "pull-request"],
    ["Back to pull requests", "pull-requests"],
    ["Open #482", "workspace-pr"],
    ["Editor", "workspace"],
    ["Schedules", "schedules"],
    ["Insights", "insights"],
    ["Passkey sign-in", "loop"],
    ["Idempotency keys", "uncommitted"],
    ["Webhook retries", "background"],
    ["Session refresh", "workspace"],
    ["Review", "changes"],
    ["Pull request", "workspace-pr"],
    ["Back to Inbox", "inbox"],
    ["Select Checkout tests", "inbox-checkout"],
    ["Select Session refresh", "inbox-session"],
    ["Review changes", "changes"],
    ["Back to Inbox", "inbox"],
    ["Select Passkey sign-in", "inbox-passkey"],
    ["Open workspace", "loop"],
  ]) {
    await go(name);
    await at(screen);
    await loaded(demo.locator(".layer img").last());
  }
  assert.equal(
    await page.evaluate(() => document.querySelector(".frame").contains(document.activeElement)),
    true,
    "Focus stays in the demo when its control goes away",
  );

  // The screenshot cards crop their captures and open them whole.
  await expect(page.locator(".bn iframe")).toHaveCount(0);
  const crops = page.locator(".bn-crop");
  for (const [index, screen] of [
    "background",
    "inbox",
    "workspace-pr",
    "pull-request",
    "uncommitted",
    "insights",
  ].entries()) {
    const image = crops.nth(index).locator(".capture-trigger img");
    await image.scrollIntoViewIfNeeded();
    await expect(image).toHaveAttribute(
      "src",
      `/screenshots/${screen}.png`,
    );
    await loaded(image);
    await crops.nth(index).locator(".capture-trigger").click();
    await page
      .getByRole("button", { name: "Close screenshot", exact: true })
      .click();
    await expect(viewer).toHaveCount(0);
  }

  const modeTitles = new Set();
  for (const name of ["Ask", "Plan", "Agent", "Review", "Autopilot"]) {
    const button = page
      .locator(".bn-modes")
      .getByRole("button", { name: new RegExp(`^${name}`) });
    await button.click();
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".bn-prompt > .on")).toHaveCount(1);
    modeTitles.add(await page.locator(".bn-prompt > .on strong").innerText());
  }
  assert.equal(modeTitles.size, 5, "Each mode shows its own panel");

  for (const width of [1440, 1024, 768, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      `Overflow at ${width}px`,
    );
    const steady = async (
      selector,
      group,
      names,
      what,
      attribute = "aria-pressed",
      value = "true",
    ) => {
      let first;
      for (const name of names) {
        const button = page
          .locator(group)
          .getByRole("button", { name: new RegExp(`^${name}`) });
        await button.click();
        await expect(button).toHaveAttribute(attribute, value);
        const box = await page.locator(selector).boundingBox();
        const size = { width: box.width, height: box.height };
        if (first)
          assert.deepEqual(size, first, `${what} must not shift at ${width}px`);
        else first = size;
      }
    };
    if (width >= 900)
      await steady(
        ".bn-stage",
        ".spots",
        ["Issues", "Insights", "Passkey sign-in"],
        "Demo",
        "aria-current",
        "page",
      );
    else
      await steady(
        ".bn-stage",
        ".pills",
        ["Autopilot", "Inbox", "Pull request", "Insights"],
        "Demo",
      );
    await steady(
      ".bn-prompt",
      ".bn-modes",
      ["Ask", "Plan", "Agent", "Review", "Autopilot"],
      "Mode card",
    );
  }

  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole("tab", { name: "Fan out", exact: true }).click();
  assert.match(await page.locator(".lf-cap").textContent(), /Every ready step/);
  assert.equal(
    await page
      .locator(".lfg .cyc")
      .first()
      .evaluate((el) => getComputedStyle(el).animationName),
    "none",
  );

  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.reload();
  await page.locator(".bn-play").scrollIntoViewIfNeeded();
  await page.getByRole("tab", { name: "Plan", exact: true }).click();
  await page.waitForFunction(
    () => document.querySelector(".lf-cap b").textContent === "Fan out.",
    null,
    { timeout: 7000 },
  );
  await page.locator(".bn-play").click();
  const paused = await page.locator(".lf-cap b").textContent();
  await page.waitForTimeout(4200);
  assert.equal(await page.locator(".lf-cap b").textContent(), paused);
  await page.locator(".bn-play").click();
  await page.waitForFunction(
    (text) => document.querySelector(".lf-cap b").textContent !== text,
    paused,
    { timeout: 7000 },
  );
  assert.deepEqual(errors, []);

  console.log(
    "Splash checks passed: downloads, captured screenshots, full-size views, modes, responsive layout, workflow animation, and the hero demo.",
  );
} finally {
  await browser?.close();
  server?.httpServer.closeAllConnections();
  if (server) await new Promise((resolve) => server.httpServer.close(resolve));
}
