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

  for (const provider of ["Claude Code", "Cursor", "Codex"]) {
    const icon = page
      .locator(".bn-agents li")
      .filter({ hasText: new RegExp(`^${provider}$`) })
      .locator("svg");
    await expect(icon).toBeVisible();
    await expect(icon).toHaveAttribute("aria-hidden", "true");
    await expect(icon.locator("path")).toHaveAttribute("fill", "currentColor");
  }

  const screens = {
    Workspace: "workspace",
    Diff: "changes",
    Autopilot: "loop",
    "Pull request": "pull-request",
    Inbox: "inbox",
    Issues: "issues",
    Schedules: "schedules",
  };
  const loaded = (image) =>
    expect
      .poll(() => image.evaluate((el) => el.complete && el.naturalWidth > 0))
      .toBe(true);
  const viewer = page.getByRole("dialog");
  let tourSize;
  for (const [name, screen] of Object.entries(screens)) {
    const button = page
      .locator(".bn-tabs")
      .getByRole("button", { name, exact: true });
    await button.click();
    await expect(button).toHaveAttribute("aria-pressed", "true");
    // The capture swaps once it has decoded.
    const image = page.locator(".bn-window .capture-trigger img");
    await expect(image).toHaveAttribute(
      "src",
      `/screenshots/${screen}.png`,
    );
    await loaded(image);
    const imageBox = await image.boundingBox();
    const size = { width: imageBox.width, height: imageBox.height };
    if (tourSize)
      assert.deepEqual(
        size,
        tourSize,
        "Tour image dimensions must not jump between screens",
      );
    else tourSize = size;
    await page.locator(".bn-window .capture-trigger").click();
    await expect(viewer).toBeVisible();
    const bounds = await viewer.locator("img").evaluate((el) => ({
      width: el.getBoundingClientRect().width,
      native: Number(el.getAttribute("width")),
      viewport: innerWidth,
    }));
    assert.ok(
      bounds.width <= bounds.native && bounds.width < bounds.viewport,
      "Full-size image must respect CSS size, not 2x pixel size",
    );
    await viewer
      .getByRole("button", { name: "Close screenshot", exact: true })
      .press("Escape");
    await expect(viewer).toHaveCount(0);
  }

  // The inbox and pull request cards crop their captures and open them whole.
  await expect(page.locator(".bn iframe")).toHaveCount(0);
  const crops = page.locator(".bn-crop");
  for (const [index, screen] of ["inbox", "pull-request"].entries()) {
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
    const steady = async (selector, group, names, what) => {
      let first;
      for (const name of names) {
        const button = page
          .locator(group)
          .getByRole("button", { name: new RegExp(`^${name}`) });
        await button.click();
        await expect(button).toHaveAttribute("aria-pressed", "true");
        const box = await page.locator(selector).boundingBox();
        const size = { width: box.width, height: box.height };
        if (first)
          assert.deepEqual(size, first, `${what} must not shift at ${width}px`);
        else first = size;
      }
    };
    await steady(".bn-stage", ".bn-tabs", Object.keys(screens), "Tour");
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

  // Without reduced motion the hero tour advances itself until clicked.
  const tour = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "no-preference",
  });
  await tour.goto(url.href);
  const tabs = tour.locator(".bn-tabs");
  await expect(tabs.locator("i")).toHaveCount(1);
  await expect(
    tabs.getByRole("button", { name: "Workspace" }),
  ).toHaveAttribute("aria-pressed", "true");
  await tabs.locator("i").dispatchEvent("animationend");
  await expect(tabs.getByRole("button", { name: "Diff" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await tabs.getByRole("button", { name: "Inbox" }).click();
  await expect(tabs.locator("i")).toHaveCount(0);
  await tour.close();

  console.log(
    "Splash checks passed: downloads, captured screenshots, full-size views, modes, responsive layout, workflow animation, and the hero tour.",
  );
} finally {
  await browser?.close();
  server?.httpServer.closeAllConnections();
  if (server) await new Promise((resolve) => server.httpServer.close(resolve));
}
