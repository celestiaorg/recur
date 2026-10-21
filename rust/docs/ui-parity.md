# UI parity

The Rust UI must look and behave like the Svelte UI: same layout metrics, colors, fonts, icons, spacing, radii, borders, hover/active/focus states, animations, scroll behavior and keyboard paths. "Close enough" is not the bar. Differences are bugs unless listed below as intentional.

## Sources of truth

- Design tokens and themes: `frontend/src/styles/_tokens.scss` (every `data-theme`), ported to `recur-ui::theme`. Colors, `color-mix` results and alpha values are computed exactly, not eyeballed.
- Component styles: the remaining `frontend/src/styles/*.scss` and each component's `<style>` block, including the cascade (rule order and specificity decide, as in the browser).
- Fonts: Inter (UI, with a 450 instance), JetBrains Mono, IBM Plex Mono and Source Code Pro (code), Space Grotesk (display), and `system-ui` (keycaps). `recur-ui` bundles the exact files Google Fonts serves the web UI (`crates/ui/assets/fonts/fetch.sh` reproduces them).
- Icons: lucide (same icon names and stroke widths as `lib/icons.ts`) and the file-icon fonts and tables in `lib/file-icons`.

## Scenes

A scene is a named, deterministic state at a fixed window size and theme. `rust/tools/ui-parity/scenes.json` is the one list: the Svelte reference harness, the Rust renderer and the widget gallery all read it. A scene may carry input steps (`move`, `down`, `up`, `click`, `key`, `type`, `wait`) replayed identically on both sides, and an `animationTime` (ms) where infinite animations freeze.

- **Reference.** `rust/tools/ui-parity/web/` mounts the real Svelte components with the frontend's stylesheet, mocked Wails bridge and e2e seed (`frontend/e2e/helpers/mock-bridge.ts`), served by its own Vite config. `capture.spec.ts` (Playwright, WebKit) replays the steps, freezes every animation (finite ones at their end), saves a PNG at device scale 2 and records every text box for the diff's text mask. The Google Fonts request is answered with the bundled font files, so both sides rasterize the same binaries and nothing touches the network.
- **Candidate.** `cargo run -p recur-ui-parity -- render` builds each scene with `recur_ui::gallery::build` in a headless GPUI window (Metal renderer, scale 2) under a gpui-base `Root`, with document focus and the window active as in the page, motion frozen at the same time, and the same steps replayed.
- **Diff.** `cargo run -p recur-ui-parity -- diff` writes heatmaps to `out/diff/` (gray equal, blue text, yellow anti-aliasing, red hard) and `out/report.json`. Channel deltas of one level are equal (8-bit compositing quantization). Other differing pixels are *text* (inside a reference text box or a file-icon glyph, which is font text), *anti-aliasing* (pixelmatch's detection, a soft edge whose value moved by at most a quarter of the local contrast, or up to three levels in a smooth region where WebKit dithers gradients), or *hard*. A scene passes when hard pixels are within its `hardTolerance` (0 unless explained below) and text plus anti-aliasing within its `tolerance`.

Run everything with `wails3 task test:rust:ui-parity` (macOS: the reference is WebKit's macOS rendering). Pieces: in `rust/tools/ui-parity`, `pnpm exec playwright test` captures (`UI_PARITY_SCENES=a,b` to filter), then `cargo run -p recur-ui-parity -- check [scene…]` renders and diffs. `cargo run -p recur-ui --example gallery` shows every scene live and interactive in any theme. Every view port adds its scenes. A crate with its own gallery registers its scene prefix in `recur_ui::gallery::EXTERNAL` and is asked first by the parity tool (`recur_editor::gallery` builds `editor/*`).

### Why WebKit

Recur ships in WKWebView, so the reference is Playwright's WebKit. It also matters for measurement: Chromium on macOS snaps boxes to whole CSS pixels, while WebKit snaps to device pixels and draws text through CoreText, which recur-ui reproduces.

## How recur-ui renders like WebKit

GPUI rounds every authored length and measured size to device pixels before layout, snaps every box edge after it, and rounds anchored offsets to whole CSS pixels. WebKit keeps fractional LayoutUnits (1/64 px) and snaps only at paint. recur-ui works around each difference rather than accepting the drift:

- **Text metrics.** `text::line_metrics` is WebKit's line box: ascent rounded and descent ceiled to 1/64, `normal` as rounded ascent + descent + gap, numeric line heights floored to 1/64, half-leading floored, widths ceiled to 1/64. Each line's box is the union of the strut and every inline run on it, each with its own font, size and half-leading.
- **Exact box sizes.** A nowrap text is a column of two invisible leaves sized through `aspect_ratio` (which reaches Taffy unrounded): one exactly as wide as the line, one making up its exact height. Wrapping text floors its measured height to the device grid and adds an exact spacer for the remainder (assuming no wrap, or the last frame's height when the text has an id). `TextStack` lays several blocks out at exact offsets.
- **Probed baselines.** GPUI snaps a box's far edge from its unrounded position. Every expected baseline and inline-box edge gets an absolutely positioned probe leaf exactly that long; its snapped far edge is where WebKit paints. Glyphs and inline boxes paint at those positions.
- **Ties.** WebKit rounds half pixels down and right, GPUI toward zero. `style::window_root` shifts the tree by 1/4096 px through `aspect_ratio` spacers; `style::anchored_at` places overlays at their exact fractional position.
- **Glyphs.** Bundled faces are rasterized through CoreGraphics with WebKit's settings (subpixel positioning and quantization, smoothing as `-webkit-font-smoothing` asks), keyed by the run's own face, which is verified against the face's cmap because GPUI's id-to-font lookup is ambiguous. `system-ui` is shaped and drawn through CoreText with the CSS weight on SF's `wght` axis and as the CoreText weight trait (fallback glyphs such as `↵` then come from the same font WebKit picks).
- **Icons.** SVGs rasterize through CoreGraphics at the exact CSS size from a snapped origin, as WebKit paints an SVG root. Monochrome icons carry coverage as alpha with the exact color. CSS masks apply per pixel column.
- **Shadows.** `shadow::with_shadow` and `shadow::box_shadow` draw blurred `box-shadow`s as WebKit's ShadowBlur: three box blurs per axis over a CSS-pixel layer, scaled up bilinearly, clipped out of the box without anti-aliasing. GPUI's own shader samples the Gaussian four times.
- **Backgrounds and borders.** GPUI paints a background and its border as two anti-aliased quads, so the background bleeds into the border's outer edge; `style::card` / `style::inset_background` shrink the background a device pixel under an opaque border, as WebKit does. `style::css_gradient` maps a diagonal CSS `linear-gradient` onto GPUI's gradient exactly (GPUI stretches diagonals by the aspect ratio).
- **Inputs.** gpui-base's editor draws typed text and the caret; recur-ui draws placeholders with its own text at `line-height: normal`, in WebKit's default `darkGray` unless the class styles `::placeholder`.

## Terminal scenes

`scenes.json` keeps terminal scenes in their own `terminal` list (recur-ui's gallery does not build them) with the bytes they replay in `terminalInputs`. Each is the dock body as `TerminalDock.svelte` renders it (the find bar when `find` is set, then the panes) around one pane on the palette surface, with `terminalTheme` naming a shipped `[[terminalTheme]]` (empty follows the app chrome).

- **Reference.** `web/scenes/Terminal.svelte` mounts the real `TerminalPane` (xterm.js with its WebGL addon; the capture fails if xterm falls back to the DOM renderer). `capture.spec.ts` answers `AttachTerminal` with the scene's bytes, and the scene is ready once xterm has parsed their trailing `OSC 0;ready` and drawn. The harness preprocesses Svelte TypeScript like `frontend/svelte.config.js` (`vitePreprocess({ script: true })`), which `TerminalPane` needs.
- **Candidate.** `recur_terminal_view::gallery` (feature `gallery`) builds the same body around a `TerminalView` whose backend replays the bytes; the parity renderer looks scenes up in both galleries. A `type` step delivers one key per update, as Playwright types, so each keystroke's effects (an incremental find) run before the next.

### How the grid renders like xterm's WebGL canvas

The grid does not draw through `recur_ui::text`: xterm.js does not lay text out in the DOM, it rasterizes each glyph into a texture atlas through the canvas 2D API and blends those textures in WebGL, and the pixels only match when that pipeline is reproduced. `recur-terminal-view` therefore keeps its own cell renderer (`atlas.rs`, `tiles.rs`); its chrome (find bar, scrollbar fades, focus rings) follows the rules below.

- **Atlas.** `TextureAtlas._drawToCache` per glyph: a device-pixel canvas `cellWidth · max(len, 2) + 4` wide and `cellHeight + 8` tall, text at the 4px padding on the ideographic baseline through CoreText (`CTFontDrawGlyphs`, as WebKit's canvas does) with the bundled faces (`recur_ui::fonts::FACES`) or the installed font of the stack, WebKit's 14° synthetic oblique for italics, then underline styles, overline and strikethrough stroked as the atlas does (including the context line width the overline leaves for the strikethrough's half-pixel offset). The texels are cropped by `_findGlyphBoundingBox` and pass through the unpremultiply / premultiply / unpremultiply of `getImageData`, `putImageData` and the texture upload.
- **Compositing.** WebGL clears to the palette background, fills the background runs, blends glyph texels in cell order with `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` on colour and alpha (so glyph edges leave the canvas partly transparent), draws the cursor, and WebKit composites the canvas as premultiplied over `.xterm`'s background. A glyph edge of coverage `a` therefore carries an extra `a(1 - a)` of that background, which no straight-alpha GPU blend can add over an arbitrary colour (it saturates on bright channels). `tiles.rs` runs the same 8-bit math on the CPU per row and paints each row as an image; a row's tile depends on it and its two neighbours (rasters spill a few pixels) and is cached by content, so scrolling and unchanged rows reuse tiles. Glyph overhang left of a cell whose resolved background (colour and background flags) differs from the previous cell's is clipped, as `GlyphRenderer` does.
- **Above the canvas.** Link underlines, the IME composition and the scrollbar slider are GPUI primitives over the tiles, as they are separate layers over the canvas in the web pane. Custom glyphs (box drawing, blocks, powerline), which xterm draws into the atlas, are painted procedurally over the tiles with their decorations in the tiles; no scene covers them yet (`PARITY.md`).

## Shell scenes

The `shell` list in `scenes.json` holds full-window scenes (`shell/…`): the titlebar, the sidebar and an empty main area, at 1280px wide. Unported routes render nothing on the Rust side, so the reference hides them too.

- **Reference.** `web/scenes/Shell.svelte` mounts the real `App.svelte` after the inits `frontend/src/main.ts` runs, over the e2e seed, with every `.main > *` hidden. `capture.spec.ts` applies the scene's `seed` edits before installing the bridge (`projects`, `loops` and `prs` upsert rows by id, mirrored into `loopById` / `prById`; `projectGroups` and `inbox` replace; `settings` merges) and its `storage` (plus `recur.theme`) as local storage before the page loads. `treeScroll` scrolls the project tree once it has loaded.
- **Candidate.** `recur_app::gallery` (feature `gallery`) builds the `Shell` over a `MockBridge` holding the same edited seed (`gallery::patch_seed`, `gallery::scene_seed`) and an in-memory `LocalStorage` with the same entries, then scrolls the tree's `ScrollHandle` once the shell has booted.
- **Harness toolchain.** The harness pins `esrap` to the frontend's version (`pnpm.overrides`): newer `esrap` prints optional snippet parameters with their `?` (`Settings.svelte`'s `themeGrid`), which WebKit cannot parse, while the app's build strips it.

Scenes: three themes (`default`, `paper`, `nord`), the collapsed rail and its flyout (`peek`, after hovering the left edge), groups (an open group, a collapsed group with a needs-you chip, a non-git project, a hidden project with Show hidden on), the tree (every status icon, open / draft / queued / merged / closed PR icons, a working copy with branch and diff, child rows with step badges and connectors, the Scheduled subgroup expanded), a hovered row with its actions, and a scrolled tree (a stuck project header with its under-fade, the bottom mask, the scrollbar after it fades).

## Content scenes

The `content` list in `scenes.json` holds the rich-content components (`markdown/…`, `mermaid/…`, `pdf/…`, `file-preview/…`, `lightbox/…`) over the fixtures in `rust/tools/ui-parity/content/` (`make_pdf.py` writes `page.pdf` with embedded Inter faces).

- **Reference.** `web/scenes/Content.svelte` mounts `MarkdownHtml`, `StreamingMarkdown` (mid-stream), `FilePreview`, `PdfPreview` (inside `.obs`, which sets `--obs-chrome-h` as Observatory does) and `AttachmentLightbox` once every font face has loaded, and is ready when images, diagrams and pages have painted.
- **Candidate.** `recur_content::gallery` (feature `gallery`) builds the same components. It installs `webhost::BlockingRenders`, so Mermaid and pdf.js calls into their hidden WebKit pages answer within the frame (the headless window runs no application loop).
- **Mermaid and PDF.** Both sides run the same mermaid.js and pdf.js in WebKit, at the window's device scale (mermaid's layout depends on it). Mermaid labels are recur-ui text at WebKit's positions, spaced to WebKit's advances (WebKit shapes SVG text at the device-scaled size, whose tracking differs); geometry rasterizes through CoreGraphics with the paint WebKit resolved. PDF pages are pdf.js's canvas pixels.

Scenes: chat markdown in `default` and `paper` (headings, lists with nesting and tasks, a table, fences in four languages, inline code, a blockquote, links, an image, long wrapping), a stream mid-reply, a Mermaid flowchart (whole-file preview) and sequence diagram (inline), one PDF page, a markdown and an SVG file preview, and the lightbox over a message.

## Rules for UI crates

To stay pixel-exact, crates built on recur-ui must:

- Call `recur_ui::init` after `gpui_kit::init`, register `recur_ui::Assets` with the application, and render each window's content inside `style::window_root`, under `style::body`. Render `TitleTooltip` (and `Toasts` where toasts show) as the last children of the window root so they inherit the body styles.
- Draw text only through `recur_ui::text` / `span` / `Text::from_spans` / `text_stack` (never GPUI's text elements), give wrapping text that can change line count an id, and set a text's line height where CSS does (`normal` is not inherited 1.45).
- Read colors from `Theme::of(cx).tokens` (or `ThemeId::tokens` outside a window) and mix with `Color::mix` / `fade` / `over`; `Color::to_rgba8` gives the 8-bit values a browser stores. Never hard-code hex outside `theme.rs`.
- Paint disabled states with `style::faded` over the backdrop (group opacity), never by fading each primitive.
- Use `style::card` for a rounded background under a 1px border, `shadow::with_shadow` / `shadow::box_shadow` for any blurred or stacked shadow, `style::css_gradient` for diagonal gradients, `style::anchored_at` for overlays, and give every absolutely positioned overlay explicit insets.
- Clip rounded children themselves: GPUI clips rectangles, so a child in a rounded `overflow: hidden` box needs its own radius or the content-mask technique the ask toast uses.
- Drive animations with `recur_ui::motion` (`progress`, `eased`, `elapsed`) so they honor reduced motion and freeze for parity captures; never read `Instant::elapsed` for visuals.
- Make focusable controls tab stops (`FocusHandle::tab_stop(true)`) and show `style::focus_ring` while `style::is_focus_visible`.
- Add a parity scene to `scenes.json` with a Svelte scene in `web/scenes/` and a builder in `crates/ui/src/gallery/scenes.rs` (or the crate's own gallery), and keep it passing.

## Intentional differences

Each is a limit of GPUI or of matching a browser's private rendering, not a choice of look:

- **Glyph subpixel phase.** WebKit positions glyphs from the box's fractional left edge; recur-ui knows only the device-pixel left edge (recovering the fraction would need about 31 probes per text), so a glyph can land in a neighboring subpixel bin. Counted as text in the diff.
- **Gradient dither.** WebKit dithers gradients; recur-ui paints them smooth. At most three levels per channel, counted as anti-aliasing.
- **Curved-edge coverage.** GPUI's SDF shader and CoreGraphics disagree by a few levels on some pixels of rounded borders and rings; a handful of such pixels can exceed the soft-edge rule and count as hard (see the allowances below).
- **Backdrop blur.** The `.palette-bg` / `.nt-bg` scrims' `backdrop-filter: blur(2px)` is not drawn: GPUI has no backdrop filters. The scrim color matches.
- **Element transforms.** GPUI cannot scale or translate an element tree, so the toast entrance's `scale(.98)` is dropped (the fade and 8px rise remain) and `translateY(-.5px)` on the ask toast's chevron becomes a device-pixel offset (exact at scale 2).
- **Typed input text and caret.** Drawn by gpui-base's editor, not recur-ui text. Captures hide carets on both sides.
- **Synthetic oblique.** The bundled code fonts have no italic faces, so WebKit slants the regular face for `font-style: italic` (TextMate comments, parameters). GPUI cannot transform glyphs, so the editor draws them upright. Counted as text in the diff.
- **Editor code text.** The editor's content is a custom element that shapes and paints lines through GPUI's text system (as CodeMirror owns its DOM); its chrome (panels, tooltips, the vim panel) goes through recur-ui text. Glyph rasterization differences in code count as text.
- **Synthetic italics.** WebKit's synthetic-oblique glyphs and CoreGraphics' differ on a couple of edge pixels per italic glyph run (the slanted top of an `i` dot, a `t` bar); every skew construction, angle and snapping variant tried leaves those pixels as they are or moves more of them. Counted as anti-aliasing (max 18 levels).
- **Frosted terminal cells.** With transparent cells (the maximized dock on a dark palette) the canvas is composited over the frosted dock, which the grid cannot read, so glyph edges miss the backdrop light described above. Opaque palettes are exact.
- **Sticky headers.** GPUI has no `position: sticky`. The sidebar lays its tree out from fixed row heights and paints group and project headers in an absolute layer at their sticky positions, with the `::before` full-bleed band and `::after` under-fade as separate elements; the bottom `mask-image` fade is a rail-colored gradient over the scrollport (identical over the rail's opaque background).
- **Pointer events during transitions.** A collapsing rail slides out over 160ms; WebKit stops hit-testing it at once (`pointer-events: none`), GPUI keeps it hit-testable until it is gone. No scene or test depends on that window.
- **Overlay scroller.** The tab strip's `scrollbar-width: thin` scroller follows macOS overlay scrollers: it shows only after wheel scrolling, so programmatic reveals do not flash it.

- **Native checkbox.** A task list's checked, disabled `<input type=checkbox>` is WebKit's native control; recur-content draws its look (fill, border and check calibrated per theme), and the edge pixels of the rounded box and check differ.
- **Inline boxes across a wrap.** When an inline code span starts a new line, WebKit carries its left padding to that line; recur-ui's text keeps the padding on the line before, so the chip sits a padding's width left (`file-preview/markdown`). The fix belongs in `recur_ui::text` line breaking.
- **Image resampling.** Scaled images (the lightbox, PDF pages stretched to their slot, the inline chart) are sampled by GPUI's bilinear texture filter where WebKit uses CoreGraphics interpolation; internal edges differ by a few levels, counted as anti-aliasing.
- **Lightbox backdrop blur.** `backdrop-filter: blur(8px)` is not drawn (no backdrop filters in GPUI); the scrim color matches.

## Hard-pixel allowances

| Scene | Hard | Cause |
|---|---|---|
| `toasts/default`, `toasts/paper` | 2, 4 | Gradient dither beside the close glyph; curved-edge coverage at plain toast corners over the shadow |
| `select/open`, `select/multiple`, `select/searchable`, `select/focus` | 2 to 22 | Curved-edge coverage of the 3px focus ring and the 2px outline at the trigger's corners |
| `icon-picker/open` | 21 | Curved-edge coverage of the trigger ring and the selected cell's corners |
| `tab-strip/overflow` | 4 | Curved-edge coverage at the scrolled chip's close button |
| `overlays/misc-paper` | 15 | Curved-edge coverage of the popover and field corners over the light shadow |
| `editor/find` | 59 | Curved-edge coverage of the focused find field's corners and ring, and the typed query, which gpui-base's editor draws |
| `editor/vim` | 132 | The vim panel's generic `monospace` resolves to Courier in both engines, but GPUI picks a heavier Courier face than WebKit, so the `:` prefix and the typed command differ |
| `shell/paper` | 6 | The far tail of the running dot's 5px amber glow over the light rail: two levels per channel past the soft-edge rule |
| `shell/tree`, `shell/hover` | 1 | Curved-edge coverage at the working copy branch chip's rounded left border |
| `shell/scrolled` | 1 | Gradient dither inside a stuck header's under-fade |
| `overlays/palette` | 388 | Keycaps centered at fractional positions inside a scrolled list land one device pixel apart (their row offset is not probed), and the list thumb's end rounds differently |
| `markdown/chat`, `markdown/paper` | 95, 131 | The native task checkbox's edges (above) and a few edge pixels of WebKit's synthesized list bullets |
| `file-preview/markdown` | 514 | An inline code chip that starts a wrapped line keeps its padding on the line before (above) |
| `pdf/page` | 15 | The page field's number, which gpui-base's editor draws (typed input text, above) |

## Results

Last full run (`hard`/`text`/`aa` are differing pixels at device scale 2; `tolerance` and `hard ok` are the allowances in `scenes.json`):

| Scene | Hard | Text | AA | Tolerance | Hard ok |
|---|---|---|---|---|---|
| `armed-button/default` | 0 | 1486 | 436 | 2120 | 0 |
| `buttons/default` | 0 | 4822 | 1624 | 7100 | 0 |
| `buttons/paper` | 0 | 4849 | 1584 | 7080 | 0 |
| `editor/code` | 0 | 13445 | 0 | 14800 | 0 |
| `editor/find` | 59 | 11525 | 440 | 13170 | 59 |
| `editor/vim` | 132 | 13510 | 224 | 15110 | 132 |
| `editor/paper` | 0 | 13033 | 0 | 14340 | 0 |
| `icon-picker/closed` | 0 | 0 | 60 | 70 | 0 |
| `icon-picker/open` | 21 | 0 | 392 | 440 | 21 |
| `icons/app` | 0 | 0 | 86 | 100 | 0 |
| `icons/file` | 0 | 0 | 0 | 0 | 0 |
| `icons/file-light` | 0 | 0 | 0 | 0 | 0 |
| `icons/lucide` | 0 | 0 | 55 | 70 | 0 |
| `icons/provider` | 0 | 0 | 0 | 0 | 0 |
| `kbd/default` | 0 | 1288 | 208 | 1650 | 0 |
| `overlays/misc` | 0 | 2865 | 126 | 3300 | 0 |
| `overlays/misc-paper` | 15 | 2945 | 222 | 3490 | 15 |
| `overlays/modal` | 0 | 0 | 201 | 230 | 0 |
| `overlays/palette` | 388 | 2194 | 584 | 3060 | 388 |
| `project-pic/default` | 0 | 165 | 351 | 570 | 0 |
| `resize-handle/focus` | 0 | 0 | 0 | 0 | 0 |
| `resize-handle/hover` | 0 | 0 | 0 | 0 | 0 |
| `ring/default` | 0 | 362 | 10 | 410 | 0 |
| `scrollbar/default` | 0 | 0 | 32 | 40 | 0 |
| `scrollbar/paper` | 0 | 0 | 40 | 50 | 0 |
| `select/closed` | 0 | 1455 | 148 | 1770 | 0 |
| `select/closed-paper` | 0 | 1508 | 250 | 1940 | 0 |
| `select/focus` | 7 | 0 | 210 | 240 | 7 |
| `select/multiple` | 2 | 1 | 212 | 240 | 2 |
| `select/open` | 2 | 1167 | 254 | 1570 | 2 |
| `select/searchable` | 22 | 1144 | 306 | 1600 | 22 |
| `settings-group/default` | 0 | 103 | 801 | 1000 | 0 |
| `settings-group/paper` | 0 | 103 | 958 | 1170 | 0 |
| `tab-strip/default` | 0 | 2309 | 145 | 2700 | 0 |
| `tab-strip/hover` | 0 | 2918 | 145 | 3370 | 0 |
| `tab-strip/overflow` | 4 | 2288 | 0 | 2520 | 4 |
| `tab-strip/paper` | 0 | 2390 | 228 | 2880 | 0 |
| `text/calibration` | 0 | 289 | 0 | 320 | 0 |
| `toasts/default` | 2 | 9073 | 18845 | 30710 | 2 |
| `toasts/paper` | 4 | 5258 | 9821 | 16590 | 4 |
| `toggles/catppuccin-latte` | 0 | 0 | 430 | 480 | 0 |
| `toggles/default` | 0 | 0 | 482 | 540 | 0 |
| `tokens/catppuccin-latte` | 0 | 0 | 0 | 0 | 0 |
| `tokens/catppuccin-mocha` | 0 | 0 | 0 | 0 | 0 |
| `tokens/coolglow` | 0 | 0 | 0 | 0 | 0 |
| `tokens/default` | 0 | 0 | 0 | 0 | 0 |
| `tokens/nord` | 0 | 0 | 0 | 0 | 0 |
| `tokens/paper` | 0 | 0 | 0 | 0 | 0 |
| `tokens/slate` | 0 | 0 | 0 | 0 | 0 |
| `tooltip/above` | 0 | 2456 | 201 | 2930 | 0 |
| `tooltip/below` | 0 | 3 | 215 | 240 | 0 |
| `tooltip/paper` | 0 | 0 | 430 | 480 | 0 |
| `terminal/prompt` | 0 | 0 | 6 | 10 | 0 |
| `terminal/paper` | 0 | 0 | 4 | 10 | 0 |
| `terminal/selection` | 0 | 0 | 6 | 10 | 0 |
| `terminal/search` | 0 | 0 | 312 | 350 | 0 |
| `terminal/scrollbar` | 0 | 0 | 0 | 0 | 0 |
| `terminal/decorations` | 0 | 0 | 6 | 10 | 0 |
| `terminal/solarized-light` | 0 | 0 | 7 | 10 | 0 |
| `terminal/solarized-dark` | 0 | 0 | 8 | 10 | 0 |
| `shell/default` | 0 | 1942 | 799 | 3020 | 0 |
| `shell/paper` | 6 | 1990 | 756 | 3030 | 6 |
| `shell/nord` | 0 | 1930 | 782 | 2990 | 0 |
| `shell/collapsed` | 0 | 598 | 52 | 720 | 0 |
| `shell/peek` | 0 | 1942 | 799 | 3020 | 0 |
| `shell/groups` | 0 | 1842 | 902 | 3020 | 0 |
| `shell/tree` | 1 | 2842 | 980 | 4210 | 1 |
| `shell/hover` | 1 | 2708 | 949 | 4030 | 1 |
| `shell/scrolled` | 1 | 2294 | 1305 | 3960 | 1 |
| `markdown/chat` | 95 | 6027 | 5525 | 12710 | 95 |
| `markdown/paper` | 131 | 6086 | 5644 | 12910 | 131 |
| `markdown/streaming` | 0 | 1875 | 77 | 2150 | 0 |
| `mermaid/flowchart` | 0 | 9173 | 1922 | 12210 | 0 |
| `mermaid/sequence` | 0 | 8076 | 16 | 8910 | 0 |
| `pdf/page` | 15 | 175 | 18357 | 20390 | 15 |
| `file-preview/markdown` | 514 | 8290 | 206 | 9350 | 514 |
| `file-preview/svg` | 0 | 0 | 0 | 0 | 0 |
| `lightbox/image` | 0 | 22 | 14684 | 16180 | 0 |

Shell tolerances: text is the titlebar and sidebar labels (glyph subpixel phase); anti-aliasing is icon, chip and ring edges and the glow, shadow and fade gradients.

Terminal tolerances: every grid difference left is synthetic-italic edge coverage (above). `terminal/search` adds the find field: curved-edge coverage at the corners of its 2px `--amber-d` ring and the query, which gpui-base's editor draws (typed input text, above).

Content tolerances: text is glyph subpixel phase in prose, code and diagram labels; anti-aliasing is image resampling (the PDF page, the lightbox, the inline chart), shape edges inside PDF pages and Mermaid strokes, and rounded fence and table corners.
