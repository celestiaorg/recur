# UI parity

The Rust UI must look and behave like the Svelte UI: same layout metrics, colors, fonts, icons, spacing, radii, borders, hover/active/focus states, animations, scroll behavior and keyboard paths. "Close enough" is not the bar. Differences are bugs unless listed below as intentional.

## Sources of truth

- Design tokens and themes: `frontend/src/styles/_tokens.scss` (every `data-theme`), ported to `recur-ui::theme`. Colors, `color-mix` results and alpha values are computed exactly, not eyeballed.
- Component styles: the remaining `frontend/src/styles/*.scss` and each component's `<style>` block.
- Fonts: Inter (UI), JetBrains Mono (code), Space Grotesk (display), at the same sizes, weights, line heights and letter spacing. Bundle the exact font files the web UI ships.
- Icons: lucide (same icon names and stroke widths as `lib/icons.ts`) and the file-icon font in `lib/file-icons`.

## Scenes

A scene is a named, deterministic app state at a fixed window size and theme, for example `sidebar/default`, `workspace/chat-streaming`, `prs/list`, `settings/providers`, `observatory/overview`.

- **Reference.** The Svelte app runs in the browser against the mocked Wails bridge and the e2e seed (`frontend/e2e/fixtures/seed.json`), the same way Playwright e2e runs. A Playwright script drives it to each scene and saves a PNG at device scale 2.
- **Candidate.** The Rust app renders the same scene from the same seed (loaded into a throwaway data dir) at the same window size and scale and saves a PNG.
- **Diff.** A small Rust tool compares the two and writes a heatmap. A scene passes at 0 differing pixels, or with a documented per-scene tolerance for anti-aliasing differences in text rasterization only.

Tooling lives in `rust/tools/ui-parity/` (harness and scene list) and is run with `wails3 task test:rust:ui-parity`. Every view port adds its scenes.

## Intentional differences

Record each one here with a reason. Expected entries: native text rendering differences (subpixel AA), native scrollbars where the web UI drew custom ones only to imitate native.
