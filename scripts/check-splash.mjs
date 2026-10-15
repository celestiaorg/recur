// Uses the frontend's existing Playwright dependency. Start the src-web dev or
// preview server, then run: node scripts/check-splash.mjs http://localhost:5174
// With no URL, serves the production build itself. Task: wails3 task test:splash.
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(
  new URL("../frontend/package.json", import.meta.url),
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
  const nativeRequests = [];
  page.on("request", (request) => {
    if (/\/wails\/runtime|\/workspace-file/.test(request.url()))
      nativeRequests.push(request.url());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  const url = new URL(process.argv[2] ?? "http://127.0.0.1:5183");

  const isDark = () =>
    page.locator(".built").evaluate((el) => el.classList.contains("dark"));
  await page.goto(url.href);
  for (const scheme of ["dark", "light"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await expect.poll(isDark).toBe(scheme === "dark");
  }
  await page.getByRole("button", { name: "◐ Charcoal" }).click();
  await page.reload();
  assert.equal(await isDark(), true, "A picked theme overrides the system");
  await page.evaluate(() => localStorage.removeItem("vocs-theme"));

  await page
    .locator(".b-hero")
    .getByRole("button", { name: /Other downloads/ })
    .click();
  const downloads = page.locator("#hero-downloads");
  await expect(downloads).toBeVisible();
  await expect(downloads.getByRole("link", { name: ".rpm" })).toHaveCount(2);
  await page.keyboard.press("Escape");
  await expect(downloads).toBeHidden();

  for (const theme of ["paper", "charcoal"]) {
    url.searchParams.set("theme", theme);
    await page.goto(url.href);
    assert.match(await page.locator("h1").innerText(), /until it’s right/);
    assert.equal(
      await page
        .locator(".built")
        .evaluate((el) => el.classList.contains("dark")),
      theme === "charcoal",
    );
    assert.equal(
      await page
        .locator(".b-wordmark")
        .first()
        .evaluate((el) => el.naturalWidth),
      720,
    );

    for (const provider of ["Claude Code", "Cursor", "Codex"]) {
      const icon = page.locator(".b-providers > div > span").filter({ hasText: new RegExp(`^${provider}$`) }).locator("svg");
      await expect(icon).toBeVisible();
      await expect(icon).toHaveAttribute("aria-hidden", "true");
      await expect(icon.locator("path")).toHaveAttribute("fill", "currentColor");
    }

    let tourSize;
    for (const name of [
      "Workspace",
      "Diff",
      "Loop",
      "Pull request",
      "Inbox",
      "Issues",
      "Schedules",
    ]) {
      const button = page
        .locator(".b-screen-tabs")
        .getByRole("button", { name, exact: true });
      await button.click();
      await expect(button).toHaveAttribute("aria-pressed", "true");
      const image = page.locator(".b-window .capture-trigger img");
      const screen = {
        Workspace: "workspace",
        Diff: "changes",
        Loop: "loop",
        "Pull request": "pull-request",
        Inbox: "inbox",
        Issues: "issues",
        Schedules: "schedules",
      }[name];
      await expect(image).toHaveAttribute(
        "src",
        `/screenshots/${screen}-${theme}.png`,
      );
      await expect(image).toBeVisible();
      await expect
        .poll(() => image.evaluate((el) => el.complete && el.naturalWidth > 0))
        .toBe(true);
      const imageBox = await image.boundingBox();
      const size = { width: imageBox.width, height: imageBox.height };
      if (tourSize)
        assert.deepEqual(
          size,
          tourSize,
          "Tour image dimensions must not jump between screens",
        );
      else tourSize = size;
      await page.locator(".b-window .capture-trigger").click();
      const viewer = page.getByRole("dialog");
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
    // The hero viewer pages through the tour like a slideshow and reuses the
    // inline caption.
    await page.locator(".b-window .capture-trigger").click();
    const slideshow = page.getByRole("dialog");
    await expect(slideshow.locator(".position")).toHaveText("7 / 7");
    await expect(slideshow.locator("header p")).toHaveText(
      await page.locator(".b-product-caption p").innerText(),
    );
    await slideshow.getByRole("button", { name: "Next screenshot" }).click();
    await expect(slideshow.locator(".position")).toHaveText("1 / 7");
    await expect(slideshow.locator("img")).toHaveAttribute(
      "src",
      `/screenshots/workspace-${theme}.png`,
    );
    await expect(slideshow.locator("header p")).toHaveText(
      await page.locator(".b-product-caption p").innerText(),
    );
    await page.keyboard.press("ArrowLeft");
    await expect(slideshow.locator(".position")).toHaveText("7 / 7");
    await page.keyboard.press("Escape");
    await expect(slideshow).toHaveCount(0);

    await expect(
      page.locator(".b-window iframe, .b-review iframe"),
    ).toHaveCount(0);
    const prImage = page.locator("#review .b-review .capture-trigger img");
    await prImage.scrollIntoViewIfNeeded();
    await expect(prImage).toHaveAttribute(
      "src",
      `/screenshots/pull-request-${theme}.png`,
    );
    await expect
      .poll(() => prImage.evaluate((el) => el.complete && el.naturalWidth > 0))
      .toBe(true);
    await page.locator("#review .b-review .capture-trigger").click();
    await page
      .getByRole("button", { name: "Close screenshot", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    for (const name of ["Ask", "Plan", "Agent", "Review", "Loop"]) {
      await page
        .locator(".b-mode-tabs")
        .getByRole("button", { name: new RegExp(name) })
        .click();
      assert.match(
        await page.locator(".b-mode-example .b-eyebrow").textContent(),
        new RegExp(`^${name}`),
      );
    }

    const appPicker = page.getByRole("group", {
      name: "App theme",
      exact: true,
    });
    const editorPicker = page.getByRole("group", {
      name: "Editor theme",
      exact: true,
    });
    await appPicker.getByRole("button", { name: "Nord", exact: true }).click();
    await editorPicker
      .getByRole("button", { name: "GitHub Light", exact: true })
      .click();
    const preview = page.frameLocator(".b-custom-window iframe");
    await expect(preview.locator("html")).toHaveAttribute("data-theme", "nord");
    await expect(preview.locator(".cm-editor").first()).toHaveCSS(
      "background-color",
      "rgb(255, 255, 255)",
    );
    assert.equal(await appPicker.locator('[aria-pressed="true"]').count(), 1);
    assert.equal(
      await editorPicker.locator('[aria-pressed="true"]').count(),
      1,
    );
    await page
      .getByRole("button", {
        name: theme === "paper" ? "◐ Charcoal" : "☀ Paper",
        exact: true,
      })
      .click();
    const switchedTheme = theme === "paper" ? "charcoal" : "paper";
    await expect(
      page.locator(".b-window .capture-trigger img"),
    ).toHaveAttribute("src", `/screenshots/schedules-${switchedTheme}.png`);
    await expect(prImage).toHaveAttribute(
      "src",
      `/screenshots/pull-request-${switchedTheme}.png`,
    );
    await expect(preview.locator("html")).toHaveAttribute("data-theme", "nord");
    await expect(preview.locator(".cm-editor").first()).toHaveCSS(
      "background-color",
      "rgb(255, 255, 255)",
    );

    // Keyboard focus must reach swatches outside the visible scroll strip.
    const aura = editorPicker.getByRole("button", {
      name: "Aura",
      exact: true,
    });
    await aura.focus();
    await page.keyboard.press("Enter");
    assert.equal(await aura.getAttribute("aria-pressed"), "true");
    assert.ok(await editorPicker.evaluate((el) => el.scrollLeft > 0));
    await expect(preview.locator(".cm-editor").first()).toHaveCSS(
      "background-color",
      "rgb(21, 20, 27)",
    );
  }

  for (const width of [1440, 1024, 768, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    assert.ok(
      await page.evaluate(
        () => document.querySelector(".built").scrollWidth <= innerWidth,
      ),
      `Overflow at ${width}px`,
    );
    let productSize;
    for (const name of [
      "Workspace",
      "Diff",
      "Loop",
      "Pull request",
      "Inbox",
      "Issues",
      "Schedules",
    ]) {
      const tab = page
        .locator(".b-screen-tabs")
        .getByRole("button", { name, exact: true });
      await tab.click();
      // The tour swaps once the next capture has decoded.
      await expect(tab).toHaveAttribute("aria-pressed", "true");
      await page
        .locator(".b-window .capture-trigger img")
        .evaluate((img) => img.decode());
      const box = await page.locator(".b-product").boundingBox();
      const size = { width: box.width, height: box.height };
      if (productSize)
        assert.deepEqual(
          size,
          productSize,
          `Tour must not shift at ${width}px`,
        );
      else productSize = size;
    }
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
  await page.locator(".b-play").scrollIntoViewIfNeeded();
  await page.getByRole("tab", { name: "Plan", exact: true }).click();
  await page.waitForFunction(
    () => document.querySelector(".lf-cap b").textContent === "Fan out.",
    null,
    { timeout: 7000 },
  );
  await page.locator(".b-play").click();
  const paused = await page.locator(".lf-cap b").textContent();
  await page.waitForTimeout(4200);
  assert.equal(await page.locator(".lf-cap b").textContent(), paused);
  await page.locator(".b-play").click();
  await page.waitForFunction(
    (text) => document.querySelector(".lf-cap b").textContent !== text,
    paused,
    { timeout: 7000 },
  );
  assert.deepEqual(errors, []);
  assert.deepEqual(
    nativeRequests,
    [],
    "Demo must not contact a native backend",
  );

  // Without reduced motion the hero tour advances itself until clicked.
  const tour = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "no-preference",
  });
  url.searchParams.delete("theme");
  await tour.goto(url.href);
  const tabs = tour.locator(".b-screen-tabs");
  await expect(
    tabs.getByRole("button", { name: "Workspace" }).locator(".b-tour-progress"),
  ).toHaveCount(1);
  await tabs.locator(".b-tour-progress").dispatchEvent("animationend");
  await expect(tabs.getByRole("button", { name: "Diff" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await tabs.getByRole("button", { name: "Inbox" }).click();
  await expect(tabs.locator(".b-tour-progress")).toHaveCount(0);
  await tour.close();

  console.log(
    "Splash checks passed: captured screenshots, full-size links, themes, visual pickers, modes, responsive layout, and loop animation.",
  );
} finally {
  await browser?.close();
  server?.httpServer.closeAllConnections();
  if (server) await new Promise((resolve) => server.httpServer.close(resolve));
}
