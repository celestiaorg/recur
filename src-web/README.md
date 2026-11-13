# Splash page

Run `pnpm run dev` or `pnpm run build` from `src-web` (requires pnpm). Run
`mise run test:splash` from the repository root to build and browser-test it.

The hero tour and PR section use real captures from `public/screenshots`.
`ProductScreenshot.svelte` reads their filenames and dimensions from the capture
manifest. Tour buttons and the Paper/Charcoal switch select matching images;
clicking a screenshot opens a dialog capped at the original CSS size and the
viewer’s available screen space. Inbox and scheduling are included in the tour.

The bundles section mixes app and editor themes from captures too:
`ThemePreview.svelte` lays the chosen editor theme's code area
(`screenshots/themes/editor-<id>.png`) over the chosen app theme's window
(`app-<key>.png`) where `themes/manifest.json` places it. Pixels that follow the
app theme (the gutter's change bars) are transparent in the editor crops. The
workflow graph stays animated and interactive.

The screenshots in `public/screenshots` are committed. `mise run capture:splash`
recaptures them from the desktop app on the typed service fakes over the demo story in
`crates/desktop/examples/capture/dataset.json`, one fresh process per screen and
theme, at 2x (macOS with a Retina display).
`mise run capture:splash -- src-web/public/screenshots themes` recaptures only the
theme set. Edit that dataset to change what the screenshots show, and keep the
editor theme list in `crates/desktop/examples/capture/main.rs` in step with
`src/lib/themes.ts` (`mise run test:splash` checks every picker theme has its
captures).
