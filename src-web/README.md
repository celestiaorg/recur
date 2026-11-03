# Splash page

Run `pnpm run dev` or `pnpm run build` from `src-web`. Both first build the
interactive demo from the Svelte app in `demo/` with its locked dependencies (requires pnpm).
Run `mise run test:splash` from the repository root to build and browser-test it.

The hero tour and PR section use real captures from `public/screenshots`.
`ProductScreenshot.svelte` reads their filenames and dimensions from the capture
manifest. Tour buttons and the Paper/Charcoal switch select matching images;
clicking a screenshot opens a dialog capped at the original CSS size and the
viewer’s available screen space. Inbox and scheduling are included in the tour.

The bundles section keeps a compact live theme preview so app/editor swatches can
still be mixed independently. It composes the app views in an isolated iframe.
The loop graph stays animated and interactive.

The showcase builds from the product dataset in `demo/showcase/dataset.json`
and shipped TextMate themes. Rebuild it after changing demo components. Generated
demo assets live in `public/demo` and are ignored by Git. The website build copies
them into `dist`.

The screenshots in `public/screenshots` are committed. The Svelte capture pipeline
that produced them was retired with the move to the Rust desktop app.
