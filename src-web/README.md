# Splash page

Run `pnpm run dev` or `pnpm run build` from `src-web` (requires pnpm). Run
`mise run test:splash` from the repository root to build and browser-test it.

The page is `src/App.svelte`: a hero with a clickable demo, four
feature sections, and a closing download card. Its copy, links and download target live in
`src/lib/site.svelte.ts`.

The hero demo and the screenshot cards use real captures from
`public/screenshots`. `AppDemo.svelte` lays clickable areas over them; check
those areas after a recapture that moves the sidebar or a toolbar. On narrow
screens the demo is four tabs instead. `ProductScreenshot.svelte` reads capture
filenames and dimensions from the manifest, and clicking one opens a dialog
capped at the original CSS size and the viewer’s available screen space. The workflow
graph stays animated and interactive.

The screenshots in `public/screenshots` are committed. `mise run capture:splash`
recaptures them from the desktop app on the typed service fakes over the demo story in
`crates/desktop/examples/capture/dataset.json`, one fresh process per screen, in
the Charcoal theme at 2x (macOS with a Retina display). Edit that dataset to
change what the screenshots show.
