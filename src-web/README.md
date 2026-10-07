# Splash page

Run `npm run dev` or `npm run build` from `src-web`. Both first build the
interactive demo using the frontend's existing locked dependencies (requires pnpm).
Run `wails3 task test:splash` from the repository root to build and browser-test it.

The hero tour and PR section use real captures from `public/screenshots`.
`ProductScreenshot.svelte` reads their filenames and dimensions from the capture
manifest. Tour buttons and the Paper/Charcoal switch select matching images;
clicking a screenshot opens a dialog capped at the original CSS size and the
viewer’s available screen space. Inbox and scheduling are included in the tour.

The bundles section keeps a compact live theme preview so app/editor swatches can
still be mixed independently. It composes the app views in an isolated iframe.
The loop graph stays animated and interactive.

The showcase and capture builds share the dedicated product dataset in
`frontend/captures/dataset.json`, service resolvers, and shipped TextMate themes. Rebuild it after changing frontend components. Generated demo assets live
in `public/demo` and are ignored by Git. The website build copies them into `dist`.

Generate product screenshots with `wails3 task capture:splash`. See
[the capture guide](../frontend/captures/README.md) for datasets, presets, and outputs.
