# UI parity

The Rust UI must look and feel like the Svelte UI: same layout, colors, fonts, icons, spacing, radii, borders, hover/active/focus states, animations, scroll behavior and keyboard paths. Minor native rendering differences (a glyph's subpixel phase, an anti-aliased edge a few levels off, a box a device pixel away) are acceptable, and parity is never bought with frame time, extra frames or extra layout passes: when matching WebKit costs speed, the faster rendering wins and the scene records the difference as an allowance. A difference a user would notice (a wrong color, size, font, spacing or behavior) is a bug. The scenes guard against regressions: each scene's tolerances sit just above its last measurement, so a real change still fails.

## Sources of truth

- Design tokens and themes: `frontend/src/styles/_tokens.scss` (every `data-theme`), ported to `recur-ui::theme`. Colors, `color-mix` results and alpha values are computed exactly, not eyeballed.
- Component styles: the remaining `frontend/src/styles/*.scss` and each component's `<style>` block, including the cascade (rule order and specificity decide, as in the browser).
- Fonts: Inter (UI, with a 450 instance), JetBrains Mono, IBM Plex Mono and Source Code Pro (code), Space Grotesk (display), and `system-ui` (keycaps). `recur-ui` bundles the exact files Google Fonts serves the web UI (`crates/ui/assets/fonts/fetch.sh` reproduces them).
- Icons: lucide (same icon names and stroke widths as `lib/icons.ts`) and the file-icon fonts and tables in `lib/file-icons`.

## Scenes

A scene is a named, deterministic state at a fixed window size and theme. `rust/tools/ui-parity/scenes.json` is the one list: the Svelte reference harness, the Rust renderer and the widget gallery all read it. A scene may carry input steps (`move`, `down`, `up`, `click`, `key`, `type`, `wait`) replayed identically on both sides, and an `animationTime` (ms) where infinite animations freeze.

- **Reference.** `rust/tools/ui-parity/web/` mounts the real Svelte components with the frontend's stylesheet, mocked Wails bridge and e2e seed (`frontend/e2e/helpers/mock-bridge.ts`), served by its own Vite config. `capture.spec.ts` (Playwright, WebKit) replays the steps, freezes every animation (finite ones at their end), waits for fonts and images, saves a PNG at device scale 2 and records every text box for the diff's text mask (text nodes, file-icon glyphs, and placeholders: an empty field's `placeholder` and string `::before` / `::after` content, measured in their own font). The Google Fonts request is answered with the bundled font files, so both sides rasterize the same binaries and nothing touches the network.
- **Candidate.** `cargo run -p recur-ui-parity -- render` builds each scene with `recur_ui::gallery::build` in a headless GPUI window (Metal renderer, scale 2) under a gpui-base `Root`, with document focus and the window active as in the page, motion frozen at the same time, and the same steps replayed. Scenes share one app, so each starts with the toast store cleared.
- **Diff.** `cargo run -p recur-ui-parity -- diff` writes heatmaps to `out/diff/` (gray equal, blue text, yellow anti-aliasing, red hard) and `out/report.json`. Channel deltas of one level are equal (8-bit compositing quantization). Other differing pixels are *text* (inside a reference text box or a file-icon glyph, which is font text), *anti-aliasing* (pixelmatch's detection, a soft edge whose value moved by at most a quarter of the local contrast, or up to three levels in a smooth region where WebKit dithers gradients), or *hard*. A scene passes when hard pixels are within its `hardTolerance` (0 unless explained below) and text plus anti-aliasing within its `tolerance`.

`cargo run --release -p recur-ui-parity -- bench [scene…]` builds the same scenes and times full frames (`UI_PARITY_BENCH_FRAMES`, 20 by default), so a rendering change can be weighed against its cost.

Run everything with `wails3 task test:rust:ui-parity` (macOS: the reference is WebKit's macOS rendering). Pieces: in `rust/tools/ui-parity`, `pnpm exec playwright test` captures (`UI_PARITY_SCENES=a,b` to filter), then `cargo run -p recur-ui-parity -- check [scene…]` renders and diffs. `cargo run -p recur-ui --example gallery` shows every scene live and interactive in any theme. Every view port adds its scenes. A crate with its own gallery registers its scene prefix in `recur_ui::gallery::EXTERNAL` and is asked first by the parity tool (`recur_editor::gallery` builds `editor/*`).

### Why WebKit

Recur ships in WKWebView, so the reference is Playwright's WebKit. It also matters for measurement: Chromium on macOS snaps boxes to whole CSS pixels, while WebKit snaps to device pixels and draws text through CoreText, which recur-ui reproduces.

## How recur-ui renders like WebKit

Taffy lays out in unrounded floats and GPUI snaps each box's absolute edges after layout, which is how WebKit snaps its 1/64 px LayoutUnits at paint. What diverges is everything GPUI rounds *before* layout: every authored length (sizes, margins, paddings, gaps, insets, max sizes) rounds to device pixels, every measured size ceils to them, and every element laid out as its own root (`prepaint_as_root`, `anchored`) starts from a snapped origin. One such rounding (a 17.4px line, an 18.75px measured bubble, a 46.2px `11vh`) moves every box after it, and the error adds up down a column. recur-ui keeps each of these exact:

- **Exact lengths (`recur_ui::exact`).** `aspect_ratio` is the one input that reaches Taffy unrounded, so a leaf one device pixel thin on one axis is exactly as long as asked on the other. `exact::vgap` / `hgap` (`vgap_at` without a window) stand in for any fractional margin, padding or gap; `exact::lu` floors a computed length to a LayoutUnit as WebKit stores it. Percentages also reach Taffy as is: a `vh` cap is a fraction of a viewport-sized box (`style::palette_scrim`).
- **Exact measured sizes.** `exact::measured` gives a custom measured element its exact size in the same layout pass: the vendored gpui-pre's `request_measured_layout_exact` passes the measured width (ceiled to a LayoutUnit) and height to Taffy unrounded, where upstream ceils both to device pixels. Box edges still snap after layout. (This replaced spacers sized from the last frame, which cost every wrapping text and chat flow an extra frame when it appeared or changed height.) A nowrap text is a column of two invisible leaves sized the same way; `TextStack` lays several blocks out at exact offsets.
- **In-tree children.** An element that places parts of itself (runs and chips of an inline flow) puts them in the layout tree as absolutely positioned children at exact offsets, not as roots at its snapped origin. Overlays attached to a trigger hang off it in the tree (`style::attached`: `top`/`bottom: 100%` plus the gap) instead of being anchored from its snapped bounds.
- **Snapped baselines.** GPUI snaps a box's far edge from its unrounded position. A text computes where each expected baseline and inline-box edge snaps from its own unrounded origin (`Window::layout_bounds_unsnapped`, vendored gpui-pre), no extra layout nodes; glyphs and inline boxes paint at those positions.
- **Text metrics.** `text::line_metrics` is WebKit's line box: ascent rounded and descent ceiled to 1/64, `normal` as rounded ascent + descent + gap, numeric line heights floored to 1/64, half-leading floored, widths ceiled to 1/64. Each line's box is the union of the strut and every inline run on it, each with its own font, size and half-leading. An inline box that starts a line brings its left margin, border and padding onto that line. A truncation drops the spaces before its ellipsis. A text field's text (`Text::field`, placeholders) counts a negative half-leading twice, as WebKit's inner editor does (measured across Inter sizes, within 1/64 px).
- **Ties.** WebKit rounds half pixels down and right, GPUI toward zero. `style::window_root` shifts the tree by 1/4096 px through `aspect_ratio` spacers; `style::anchored_at` places free-standing overlays at their exact fractional position.
- **Flex.** CSS takes `flex: 1`'s zero basis as the item's border box without flooring it at padding and border; Taffy floors it, so padded items share space unequally. `style::flex_share` puts the share on an unpadded wrapper. GPUI does not collapse margins: ports collapse them by hand (a child's top margin into its parent's).
- **Paint order.** GPUI paints a box's border after its children, CSS before them, so anything that must paint over a parent's border (a `z-index` overlay, its shadow) paints `deferred`; one that must stay clipped by its scroll container (a `z-index` popover inside `overflow: auto`) paints through `style::deferred_clipped`. GPUI clips rectangles: a child in a rounded `overflow: hidden` box rounds its own corners (a menu's search header, a palette's footer).
- **Glyphs.** `ui-monospace` in a mono stack is SF Mono, as in WebKit (`fonts::UI_MONOSPACE`), so a glyph the bundled subset lacks (`✓`) falls back to the same face. Bundled faces are rasterized through CoreGraphics with WebKit's settings (subpixel positioning and quantization, smoothing as `-webkit-font-smoothing` asks), keyed by the run's own face, which is verified against the face's cmap because GPUI's id-to-font lookup is ambiguous. `system-ui` is shaped and drawn through CoreText with the CSS weight on SF's `wght` axis and as the CoreText weight trait (fallback glyphs such as `↵` then come from the same font WebKit picks).
- **Icons.** SVGs rasterize through CoreGraphics at the exact CSS size from a snapped origin, as WebKit paints an SVG root. Monochrome icons carry coverage as alpha with the exact color. CSS masks apply per pixel column.
- **Shadows.** `shadow::with_shadow` and `shadow::box_shadow` draw blurred `box-shadow`s as WebKit's ShadowBlur: three box blurs per axis over a CSS-pixel layer, scaled up to device pixels by CoreGraphics as the clip mask WebKit draws it through (`clipToImageBuffer`; its resampling is sharper than bilinear, which left the far tails of 5px glows a few levels off), clipped out of the box without anti-aliasing. GPUI's own shader samples the Gaussian four times. `with_shadow` is a shrinkable column, so a max height around it reaches the content.
- **Backgrounds, borders and gradients.** The vendored renderer blends a quad's border into its background premultiplied (`vendor/README.md`), so a border over a transparent background anti-aliases as CoreGraphics paints it. List markers (`disc`, `circle`) are CoreGraphics ellipses at their fractional position (`icon::paint_svg_at`), as WebKit fills and strokes them. GPUI paints a background and its border as two anti-aliased quads, so the background bleeds into the border's outer edge; `style::card` / `style::inset_background` shrink the background a device pixel under an opaque border, as WebKit does. GPUI's gradient shader dithers by ±2 levels where WebKit paints smooth, so every CSS `linear-gradient` goes through `style::css_gradient` / `gradient_fill` / `paint_css_gradient`, which rasterize its device pixels (premultiplied sRGB interpolation, exact angle and gradient line) into a cached image.
- **Hover transitions.** `.hover(..)` styles snap. A CSS `transition` on a hover color goes through `motion::hover`, which eases a 0 to 1 mix over the transition's duration and reverses from where it is; it settles for parity captures.
- **Inputs.** gpui-base's editor keeps the caret and selection; recur-ui draws placeholders and the typed value with its own text at `line-height: normal` (`Text::field`), placeholders in WebKit's default `darkGray` unless the class styles `::placeholder`. Masked values and the terminal find bar's query (unsmoothed on the dock's translucent layer) stay gpui-base's glyphs.

Diagnose a layout difference by comparing positions, not heatmaps: `UI_PARITY_BOXES=1 UI_PARITY_SCENES=<scene> pnpm exec playwright test` writes every element's fractional border box and every text node's rect to `out/reference/<scene>.boxes.txt`.

## Terminal scenes

`scenes.json` keeps terminal scenes in their own `terminal` list (recur-ui's gallery does not build them) with the bytes they replay in `terminalInputs`. Each is the dock body as `TerminalDock.svelte` renders it (the find bar when `find` is set, then the panes) around one pane on the palette surface, with `terminalTheme` naming a shipped `[[terminalTheme]]` (empty follows the app chrome).

- **Reference.** `web/scenes/Terminal.svelte` mounts the real `TerminalPane` (xterm.js with its WebGL addon; the capture fails if xterm falls back to the DOM renderer). `capture.spec.ts` answers `AttachTerminal` with the scene's bytes, and the scene is ready once xterm has parsed their trailing `OSC 0;ready` and drawn. The harness preprocesses Svelte TypeScript like `frontend/svelte.config.js` (`vitePreprocess({ script: true })`), which `TerminalPane` needs.
- **Candidate.** `recur_terminal_view::gallery` (feature `gallery`) builds the same body around a `TerminalView` whose backend replays the bytes; the parity renderer looks scenes up in both galleries. A `type` step delivers one key per update, as Playwright types, so each keystroke's effects (an incremental find) run before the next.

### How the grid renders like xterm's WebGL canvas

The grid does not draw through `recur_ui::text`: xterm.js does not lay text out in the DOM, it rasterizes each glyph into a texture atlas through the canvas 2D API and blends those textures in WebGL, and the pixels only match when that pipeline is reproduced. `recur-terminal-view` therefore keeps its own cell renderer (`atlas.rs`, `tiles.rs`); its chrome (find bar, scrollbar fades, focus rings) follows the rules below.

- **Atlas.** `TextureAtlas._drawToCache` per glyph: a device-pixel canvas `cellWidth · max(len, 2) + 4` wide and `cellHeight + 8` tall, text at the 4px padding on the ideographic baseline through CoreText (`CTFontDrawGlyphs`, as WebKit's canvas does) with the bundled faces (`recur_ui::fonts::FACES`) or the installed font of the stack, WebKit's 14° synthetic oblique for italics, then underline styles, overline and strikethrough stroked as the atlas does (including the context line width the overline leaves for the strikethrough's half-pixel offset). The texels are cropped by `_findGlyphBoundingBox` and pass through the unpremultiply / premultiply / unpremultiply of `getImageData`, `putImageData` and the texture upload.
- **Compositing.** WebGL clears to the palette background, fills the background runs, blends glyph texels in cell order with `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` on colour and alpha (so glyph edges leave the canvas partly transparent), draws the cursor, and WebKit composites the canvas as premultiplied over `.xterm`'s background. A glyph edge of coverage `a` therefore carries an extra `a(1 - a)` of that background, which no straight-alpha GPU blend can add over an arbitrary colour (it saturates on bright channels). `tiles.rs` runs the same 8-bit math on the CPU per row and paints each row as an image; a row's tile depends on it and its two neighbours (rasters spill a few pixels) and is cached by content, so scrolling and unchanged rows reuse tiles. Glyph overhang left of a cell whose resolved background (colour and background flags) differs from the previous cell's is clipped, as `GlyphRenderer` does.
- **Above the canvas.** Link underlines, the IME composition and the scrollbar slider are GPUI primitives over the tiles, as they are separate layers over the canvas in the web pane. Custom glyphs (box drawing, blocks, shades, powerline, braille) are drawn into the atlas canvas as `CustomGlyphRasterizer.ts` draws them, with the same canvas calls on CoreGraphics (`custom_glyphs::Canvas`), so they composite like text (`terminal/glyphs`).

## Shell scenes

The `shell` list in `scenes.json` holds full-window scenes (`shell/…`): the titlebar, the sidebar and an empty main area, at 1280px wide. Unported routes render nothing on the Rust side, so the reference hides them too.

- **Reference.** `web/scenes/Shell.svelte` mounts the real `App.svelte` after the inits `frontend/src/main.ts` runs, over the e2e seed, with every `.main > *` hidden. `capture.spec.ts` applies the scene's `seed` edits before installing the bridge (`projects`, `loops` and `prs` upsert rows by id, mirrored into `loopById` / `prById`; `projectGroups` and `inbox` replace; `settings` merges) and its `storage` (plus `recur.theme`) as local storage before the page loads. `treeScroll` scrolls the project tree once it has loaded.
- **Candidate.** `recur_app::gallery` (feature `gallery`) builds the `Shell` over a `MockBridge` holding the same edited seed (`gallery::patch_seed`, `gallery::scene_seed`) and an in-memory `LocalStorage` with the same entries, then scrolls the tree's `ScrollHandle` once the shell has booted.
- **Harness toolchain.** The harness pins `esrap` to the frontend's version (`pnpm.overrides`): newer `esrap` prints optional snippet parameters with their `?` (`Settings.svelte`'s `themeGrid`), which WebKit cannot parse, while the app's build strips it.

Scenes: three themes (`default`, `paper`, `nord`), the collapsed rail and its flyout (`peek`, after hovering the left edge), groups (an open group, a collapsed group with a needs-you chip, a non-git project, a hidden project with Show hidden on), the tree (every status icon, open / draft / queued / merged / closed PR icons, a working copy with branch and diff, child rows with step badges and connectors, the Scheduled subgroup expanded), a hovered row with its actions, and a scrolled tree (a stuck project header with its under-fade, the bottom mask, the scrollbar after it fades). With the detail route open (clicked): the target-branch menu open (`shell/base-select`, the `.sel.open` cascade over the titlebar trigger) and the linked-issue button with its square-left picker (`shell/issue`). The detail route's main area stays hidden behind `.main > *` there.

## Content scenes

The `content` list in `scenes.json` holds the rich-content components (`markdown/…`, `mermaid/…`, `pdf/…`, `file-preview/…`, `lightbox/…`) over the fixtures in `rust/tools/ui-parity/content/` (`make_pdf.py` writes `page.pdf` with embedded Inter faces).

- **Reference.** `web/scenes/Content.svelte` mounts `MarkdownHtml`, `StreamingMarkdown` (mid-stream), `FilePreview`, `PdfPreview` (inside `.obs`, which sets `--obs-chrome-h` as Observatory does) and `AttachmentLightbox` once every font face has loaded, and is ready when images, diagrams and pages have painted.
- **Candidate.** `recur_content::gallery` (feature `gallery`) builds the same components. It installs `webhost::BlockingRenders`, so Mermaid and pdf.js calls into their hidden WebKit pages answer within the frame (the headless window runs no application loop).
- **Mermaid and PDF.** Both sides run the same mermaid.js and pdf.js in WebKit, at the window's device scale (mermaid's layout depends on it). Mermaid labels are recur-ui text at WebKit's positions, spaced to WebKit's advances (WebKit shapes SVG text at the device-scaled size, whose tracking differs); geometry rasterizes through CoreGraphics with the paint WebKit resolved. PDF pages are pdf.js's canvas pixels.

Scenes: chat markdown in `default` and `paper` (headings, lists with nesting and tasks, a table, fences in four languages, inline code, a blockquote, links, an image, long wrapping), a stream mid-reply, a Mermaid flowchart (whole-file preview) and sequence diagram (inline), one PDF page, a markdown and an SVG file preview, and the lightbox over a message.

## Chat scenes

The `chat` list in `scenes.json` holds the agent chat (`chat/…`) over the fixtures in `rust/tools/ui-parity/content/chat.json`: `WorkspaceChatItems` rows and cockpit facts per scene, reduced on both sides by the same reducers (`mapChatItems` / `applyStream` / `mergeLoopCockpit` and their `recur_engine::view` ports).

- **Reference.** `web/scenes/Chat.svelte` mounts `AgentChat` as the workspace dock does (compact, the Loop or agent tab's copy, the `view` store on `detail` so inline permission and question cards show) with the dock's mode toolbar on the `mode-*` scenes (the chat mode `Select`, then `ModelPickers` or `LoopPolicyPickers`), and `AppChat` under the Briefing body's `--obs-chrome-h`. It is ready once the model catalog has loaded and the prompt index and file icons have painted.
- **Candidate.** `recur_view_chat::gallery` (feature `gallery`) builds the same `AgentChat` / `AppChat` over `MockBridge::fresh` and clears the transcript UI cache per scene (scenes share one app).

Scenes: the empty chat; a `present_workspace` result with its brief card in the tool row and on the turn footer (`brief-card`, `default` and `paper`); a finished turn with a file chip, a thought, tool calls with results (group opened by a click), markdown with a fence, a list, a link and the turn footer, in `default` and `paper`; a reply streaming mid-turn with the turn clock; inline permission, question, plan and fan-out cards; the composer with file chips and the `/` menu open; each mode's composer toolbar (`ask`, `plan`, `agent`, `review`, `loop`, and `agent` in `paper`); the model picker open; and Ask Recur in `default` and `paper`.

## Review scenes

The `review` list in `scenes.json` holds the review surface (`review/…`) over the fixtures in `rust/tools/ui-parity/content/review.json`: base, working and commit diffs, dirty paths, findings, commits, and replies added to PR #482's seeded thread.

- **Reference.** `web/scenes/Review.svelte` mounts `ReviewSurface` as the Observatory does (`capture.spec.ts` answers `DiffVsBase`, `DiffWorking`, `ListCommits` and `CommitDiff` from the fixtures), and for the thread scenes a PR-mode `DiffFile` inside PR detail's files pane (`.prpane > .diffwrap > .diffscroll > .cfile`) with PR #482 selected. The large scene generates its 40 files the way `recur_view_review::gallery::large_files` does.
- **Candidate.** `recur_view_review::gallery` (feature `gallery`) builds the same `ReviewSurface` or a `FileStack` with one card over `MockBridge::fresh`, with the same local storage (`recur.prDiffLayout`, `recur.reviewFileListMode`).

Scenes: the unified stack with the tree file list, findings and dirty marks in `default` and `paper`; split layout; a collapsed hunk (clicked); a collapsed card (clicked); the flat file list after selecting a file (a stuck card header, the list's reveal); a line-note thread with replies and reactions in `default` and `paper`; the commits tab with its uncommitted row in `default` and `paper`; and a 40-file mixed-size diff windowed into stubs and hydrated cards.

## Observatory scenes

The `observatory` list in `scenes.json` holds the Observatory (`observatory/…`) over `rust/tools/ui-parity/content/observatory.json`: a worktree tree with git status, file bodies and their base versions, an outline, artifacts, a PNG, a PDF, a terminal session with its output, and each scene's editor session.

- **Reference.** `web/scenes/Observatory.svelte` mounts `Observatory` in a `.win` (which sets `--chrome-foot-h`) after `loadInitial`, opening the terminal dock (below or beside) where the scene asks; `capture.spec.ts` answers the tree, file, git, outline, artifact, session and terminal calls from the fixture (`observatoryResolvers`) and serves `/workspace-file` from its assets.
- **Candidate.** `recur_view_observatory::gallery` (feature `gallery`) builds the same `Observatory` over `MockBridge::fresh` with the same resolvers, resets the crate's app globals per scene, and renders PDF pages synchronously (`BlockingRenders`).

Scenes: the explorer and code editor in `default` and `paper`; an overflowing tab strip; the merge view; the review tab; the terminal dock below, beside and maximized (clicked); Quick Open (⌘P); the artifacts panel (clicked); a PDF; an image; and the Source Control panel (clicked).

## Workspace scenes

The `workspace` list in `scenes.json` holds the detail route (`workspace/…`) bound to LOOP-2231 over `rust/tools/ui-parity/content/workspace.json` (each scene's chat rows by `items` from `content/chat.json`, chat tabs, sessions, facts, agents and findings) and the Observatory fixture of the scene's `session`.

- **Reference.** `web/scenes/Workspace.svelte` mounts `WorkspaceDetail` as App.svelte does in `.main` after `loadInitial` and `openWorkspace`, overriding Shell.svelte's global `.main > *` rule so the route shows; `capture.spec.ts` answers the chat, session, fact, finding and agent calls from the fixture (`workspaceResolvers`) on top of `observatoryResolvers`, after the scene's `seed` edits.
- **Candidate.** `recur_view_workspace::gallery` (feature `gallery`) builds the same `WorkspaceDetail` over a `MockBridge` with the edited seed and the same resolvers, resets the Observatory's and the chat's app globals per scene, then opens the workspace after `load_initial`.
- **Between scenes.** The parity binary opens, activates and removes a blank window after each scene, so the previous scene's views are released (and their teardown writes land) before the next scene installs its storage; the test platform otherwise keeps a removed window active.

Scenes: the chat with the Observatory in `default` and `paper`; many chat tabs overflowing the strip (`tabs`, 1180 wide so the active chip is revealed the same way on both sides, see `PARITY.md`); the plan panel and the findings panel (their dock toggles clicked); and a running Loop with the maestro dock in `default` and `paper`.

## PR scenes

The `prs` list in `scenes.json` holds the PR routes (`prs/…`): the dashboard and PR detail in the whole window as Shell.svelte mounts it, over the e2e seed with the edits of `rust/tools/ui-parity/content/prs.json` (`list` adds six GitHub PRs covering every group, status and attention badge; the detail fixtures patch PR #482 into each titlebar state: behind its base, draft, failing checks while blocked, blocked, conflicts without a workspace, queued, merge queue, merged, two pending line comments).

- **Reference.** `web/scenes/Prs.svelte` mounts `App` and, once the tree loads, opens PR detail (`pr`, the dock closed with `dock: "closed"`, then `tab` and `review` three frames later, after PRDetail's per-PR reset lands on Conversation) or the dashboard (`seg`), overriding Shell.svelte's global `.main > *` rule so the route shows. `capture.spec.ts` applies the fixture's seed edits and answers its resolvers (`prsFixture`, `prsResolvers`), and aborts avatar fetches so faces fall back to initials on both sides.
- **Candidate.** `recur_view_prs::gallery` (feature `gallery`) builds the desktop's registry (chat, review, Observatory, workspace, PRs) and a `Shell` over a `MockBridge` with the same seed edits and resolvers, then navigates the same way after boot (the tab and review modal three frames later).

Scenes: the dashboard on All in `default` and `paper`, wide (1800, people and why on the row), with the sort menu open and with a people popout; PR detail on Conversation in `default` and `paper`, Files changed, Commits and Checks; the titlebar in each merge state (Update with main beside Merge, Ready for review, Fix checks, Merge blocked, Conflicts, Queued, Add to merge queue, merged, two pending comments in `default` and `paper`), with the merge, update and review menus open, and confirming a merge; the review modal with pending chips (Request changes in `default`, Approve in `paper`); and the header's people popout.

## Settings scenes

The `settings` list in `scenes.json` holds full-window scenes (`settings/…`) of the Settings route at 1280×800, in `default` and `paper`: one per pane, the Add provider modal, the Cursor SDK install mid-download, the default policy menu and its help, the rail's usage meter and the keybinding menu.

- **Reference.** `web/scenes/SettingsPage.svelte` mounts `App.svelte` as the shell scenes do, opens Settings on the scene's `section` and runs its `act` (a click on Add provider, Install SDK followed by a `cursor.install` progress event, the policy or help button, the usage meter, `openKeybindingMenu`). Each scene names a fixture in `content/settings.json`: edits to the e2e seed and canned resolvers; `{"$pending": true}` leaves a call unanswered (the install in flight).
- **Candidate.** `recur_view_settings::gallery` (feature `gallery`) builds the desktop's registry and `Shell` over a `MockBridge` with the same fixture, opens Settings on `section` once the state has booted, and three frames later runs the same `act` (the policy, help and usage scenes click instead, through `steps`).

## Home, briefing and project scenes

The `home`, `briefing` and `projects` lists in `scenes.json` hold full-window scenes (`home/…`, `briefing/…`, `projects/…`) of Welcome, the New workspace and New project modals, the Inbox, Issues, Schedules with the schedule editor, and project settings, in `default` and `paper`. Each names a fixture in `content/views.json` (edits to the e2e seed and canned replies: `first-run` and `no-workspaces` empty the lists; `issues`, `issues-board` and `schedules` answer `ListAssignedIssues`, `GetIssuesView`, `ListScheduledPrompts` and `PreviewScheduledPrompt` with fixed dates) and a `route`.

- **Reference.** `web/scenes/Views.svelte` mounts `App.svelte` over the edited seed and, once the rail is up, navigates to the route (`welcome`, `briefing`, `issues`, `schedules`, `newtask`, `newproject`, `project:{id}`), hiding every main-area view but the routed one. Steps then click (picker menus, a project section, the Schedules page's New schedule) and park the pointer. Avatar fetches are aborted, as for the PR scenes.
- **Candidate.** `recur_view_core::gallery::build_in` (feature `gallery`; the home, briefing and projects galleries call it) builds the desktop's registry (keeping only the routed view, as `Views.svelte` hides the rest of `.main`) and a `Shell` from the `SceneHost` `ui-parity` installs, over a `MockBridge` with the same fixture and storage, navigates the same way after boot and replays the steps. Each scene starts from fresh module state (the Issues view cache, expanded schedule rows).

Scenes: Welcome on first run, its modes and its completed state; New project empty and with a typed path; New workspace in agent and loop mode with each picker open (repository, base, mode, model, flags, loop, policy, the policy help) and the slash menu; the Inbox with every bucket and Elsewhere, with the Ask dock closed and as a drawer at 1000px; Issues as a list and a board; Schedules across projects and the schedule editor; project settings General (also 1500px tall), Run (also 1800px tall), Schedules, Pull requests, Skills, MCP and Danger zone.

## Palette scenes

The `palettes` list in `scenes.json` holds full-window scenes (`palettes/…`) of every palette over the shell, in `default` and `paper`: Go to (⌘K) with its overflowing list, a filter and no matches; Run Command (⌘⇧P) for `src/main.go` with Recent from `recur.commandUsage`, and filtered; Open in on LOOP-2231 and filtered; Run with five commands and a pinned default (fixture `palette-runs` in `content/views.json`) and filtered; the language picker for `src/main.go` and filtered; and the shortcuts list and filtered.

- **Reference.** `web/scenes/Views.svelte` routes `palette`, `commands:{path}:{scope}`, `shortcuts`, `openin:{workspace}`, `run:{workspace}` and `syntax:{path}`: a route's path stands in for the focused editor (`setEditorFocusProvider`), and Open in and Run open 300ms after the workspace lands (landing closes palettes). `.main > *` stays hidden, as for the other view scenes.
- **Candidate.** `recur_view_palettes::gallery` builds a `Shell` over the same fixture with only the palettes registered (the main area is empty, as the reference hides it), sets the same editor focus provider, and opens Open in and Run from a 300ms timer that the scenes' `wait` step reaches.

## Rules for UI crates

To stay pixel-exact, crates built on recur-ui must:

- Call `recur_ui::init` after `gpui_kit::init`, register `recur_ui::Assets` with the application, and render each window's content inside `style::window_root`, under `style::body`. Render `TitleTooltip` (and `Toasts` where toasts show) as the last children of the window root so they inherit the body styles.
- Draw text only through `recur_ui::text` / `span` / `Text::from_spans` / `text_stack` (never GPUI's text elements), and set a text's line height where CSS does (`normal` is not inherited 1.45).
- Never author a length GPUI would round. Any margin, padding, gap or size that is not a multiple of the device pixel (a line height, a centering offset, `11vh`, a measured width) is an `exact::vgap` / `hgap` (or a percentage of a definite box), never `.mt(px(0.95))` or `.h(px(17.4))`; a box whose parts add up to its size gets no authored size. Lay custom measured elements out through `exact::measured` and give them an id; place their parts in the layout tree, not with `prepaint_as_root`.
- Attach menus and popovers to their trigger with `style::attached` (inside `deferred`); keep `style::anchored_at` for overlays at a free window position (a pointer, a caret).
- Wrap padded `flex: 1` items in `style::flex_share`, collapse adjoining vertical margins by hand, and paint anything with a `z-index` over its parent's border `deferred`.
- Read colors from `Theme::of(cx).tokens` (or `ThemeId::tokens` outside a window) and mix with `Color::mix` / `fade` / `over`; `Color::to_rgba8` gives the 8-bit values a browser stores. Never hard-code hex outside `theme.rs`.
- Paint disabled states with `style::faded` over the backdrop (group opacity), never by fading each primitive.
- Use `style::card` for a rounded background under a 1px border, `shadow::with_shadow` / `shadow::box_shadow` for any blurred or stacked shadow, `style::css_gradient` / `gradient_fill` / `paint_css_gradient` for every gradient (never GPUI's `linear_gradient`), and give every absolutely positioned overlay explicit insets.
- Clip rounded children themselves: GPUI clips rectangles, so a child in a rounded `overflow: hidden` box needs its own radius or the content-mask technique the ask toast uses.
- Drive animations with `recur_ui::motion` (`progress`, `eased`, `elapsed`) so they honor reduced motion and freeze for parity captures; never read `Instant::elapsed` for visuals. Where CSS transitions a hover color, use `motion::hover` instead of `.hover(..)`.
- Make focusable controls tab stops (`FocusHandle::tab_stop(true)`) and show `style::focus_ring` while `style::is_focus_visible`.
- Add a parity scene to `scenes.json` with a Svelte scene in `web/scenes/` and a builder in `crates/ui/src/gallery/scenes.rs` (or the crate's own gallery), and keep it passing.

## Intentional differences

Each is a limit of GPUI or of matching a browser's private rendering, not a choice of look:

- **Gradient dither.** recur-ui paints gradients smooth. WebKit paints opaque gradients smooth too, but dithers some that fade to transparent (the sidebar's under-fades) by a level or two; counted as anti-aliasing, except where the noise lands on a faint edge.
- **Glyph subpixel phase.** recur-ui places glyphs from the box's unrounded left edge, as WebKit does; CoreGraphics' subpixel quantization still lands a few glyphs in a neighboring bin. Counted as text in the diff.
- **Curved-edge coverage.** GPUI's SDF shader and CoreGraphics disagree by a few levels on some pixels of rounded borders and rings; a handful of such pixels can exceed the soft-edge rule and count as hard (see the allowances below).
- **Backdrop blur.** The `.palette-bg` / `.nt-bg` scrims' `backdrop-filter: blur(2px)` is not drawn: GPUI has no backdrop filters. The scrim color matches.
- **Element transforms.** GPUI cannot scale or translate an element tree, so the toast entrance's `scale(.98)` is dropped (the fade and 8px rise remain) and `translateY(-.5px)` on the ask toast's chevron becomes a device-pixel offset (exact at scale 2).
- **Caret.** Drawn by gpui-base's editor. Captures hide carets on both sides.
- **Synthetic oblique.** The bundled code fonts have no italic faces, so WebKit slants the regular face for `font-style: italic` (TextMate comments, parameters). GPUI cannot transform glyphs, so the editor draws them upright. Counted as text in the diff.
- **Editor code text.** The editor's content is a custom element that shapes and paints lines through GPUI's text system (as CodeMirror owns its DOM); its chrome (panels, tooltips, the vim panel) goes through recur-ui text. Glyph rasterization differences in code count as text.
- **Synthetic italics.** WebKit's synthetic-oblique glyphs and CoreGraphics' differ on a couple of edge pixels per italic glyph run (the slanted top of an `i` dot, a `t` bar); every skew construction, angle and snapping variant tried leaves those pixels as they are or moves more of them. Counted as anti-aliasing (max 18 levels).
- **Frosted terminal cells.** With transparent cells (the maximized dock on a dark palette) the canvas is composited over the frosted dock, which the grid cannot read, so glyph edges miss the backdrop light described above. Opaque palettes are exact.
- **Sticky headers.** GPUI has no `position: sticky`. The review stack paints a card's stuck `.df-head` in an absolute layer inside the card, and the review file list lays its rows out from fixed heights with folder rows in a sticky layer. The sidebar lays its tree out from fixed row heights and paints group and project headers in an absolute layer at their sticky positions, with the `::before` full-bleed band and `::after` under-fade as separate elements; the bottom `mask-image` fade is a rail-colored gradient over the scrollport (identical over the rail's opaque background).
- **Pointer events during transitions.** A collapsing rail slides out over 160ms; WebKit stops hit-testing it at once (`pointer-events: none`), GPUI keeps it hit-testable until it is gone. No scene or test depends on that window.
- **Overlay scroller.** The tab strip's `scrollbar-width: thin` scroller follows macOS overlay scrollers: it shows only after wheel scrolling, so programmatic reveals do not flash it.

- **Native checkbox.** A task list's checked, disabled `<input type=checkbox>` is WebKit's native control; recur-content draws its look (fill, border and check calibrated per theme), and the edge pixels of the rounded box and check differ.
- **Image resampling.** Scaled images (the lightbox, PDF pages stretched to their slot, the inline chart) are sampled by GPUI's bilinear texture filter where WebKit uses CoreGraphics interpolation; internal edges differ by a few levels, counted as anti-aliasing.
- **Lightbox backdrop blur.** `backdrop-filter: blur(8px)` is not drawn (no backdrop filters in GPUI); the scrim color matches.
- **LSP documentation links and signature indent.** recur-ui text has no per-side inline border, inline underline color or `text-indent`: a link in hover or completion documentation draws its `border-bottom` (cyan at 45%) as a full-cyan underline, and a signature that wraps in signature help starts its continuation lines at the left edge instead of under CodeMirror's `1em hanging` indent.

## Hard-pixel allowances

Every other scene allows no hard pixels. These are the known minor differences; each scene's tolerance sits just above its measurement so a regression still fails.

| Scene | Hard | Cause |
|---|---|---|
| `toasts/paper` | 4 | Curved-edge coverage at plain toast corners over the shadow |
| `select/open`, `select/multiple`, `select/searchable`, `select/focus` | 2, 2, 2, 7 | Curved-edge coverage of the 3px focus ring and the 2px outline at the trigger's corners |
| `icon-picker/open` | 3 | The trigger's bottom corners where its 3px amber spread ring meets the box: WebKit leaves the corner pixel inside the box's aliased clip-out at 55 where recur-ui paints 94. The pixels differed by the same amount before the shadow upscale fix, classified as anti-aliasing until their neighbors matched |
| `overlays/misc-paper` | 15 | Curved-edge coverage of the popover and field corners over the light shadow |
| `editor/find` | 41 | Curved-edge coverage of the focused find field's corners and ring, and the typed query, which the editor's panel field draws |
| `editor/vim` | 132 | The vim panel's generic `monospace` resolves to Courier in both engines, but GPUI picks a heavier Courier face than WebKit, so the `:` prefix and the typed command differ |
| `shell/tree`, `shell/hover` | 1 | Curved-edge coverage at the working copy branch chip's rounded left border |
| `shell/scrolled` | 2 | WebKit dithers a stuck header's under-fade by a level; where the noise lands on a child row's faint connector edge it exceeds the soft-edge rule (recur-ui paints the fade smooth) |
| `markdown/chat`, `markdown/paper` | 82, 116 | The native task checkbox's shading and edges (above), and a pixel per list marker |
| `review/collapsed` | 8 | The collapsed `.df-head`'s bottom corners: a bottom-only 1px border around a radius-8 corner tapers to nothing, and its elliptical inner edge (radii 8 and 7) is approximated in GPUI's quad shader where CoreGraphics fills the exact area between the curves (2 to 3 levels past the soft-edge rule on 4 pixels per corner) |
| `pdf/page` | 10 | The page field's number |
| `observatory/terminal-max` | 2 | Frosted terminal cells (above): glyph edges over the maximized dock's frost |
| `observatory/pdf` | 257 | Glyph edges inside the page raster, which GPUI's bilinear filter resamples (image resampling, above) past the soft-edge rule, and the page field's number |
| `workspace/narrow` | 22 | The Files status bar icons at a sub-pixel offset |
| `prs/*` (most), `prs/people-detail` | 1 | An avatar's edge at a fractional offset in the header's people stack: WebKit clips the avatar at its unsnapped position |
| `prs/actions-checks` | 5 | The avatar edge, and the Fix checks button's corners at a fractional x |
| `prs/actions-review-menu` | 5 | The avatar edge, and the avatar ring under the menu's shadow |
| `prs/actions-merged` | 14 | The classic thumb is a pixel shorter: GPUI lays the merged conversation out 989.5px tall where WebKit has 989.28, and `scrollHeight` rounds across the boundary; the avatar edge |
| `settings/general`, `settings/usage`, `settings/keybinding-menu` | 0, 0, 8 | Field corners at a fractional height (the nav search box), and a pixel of the concurrency field's value |
| `settings/*-paper` (general, appearance, mcp, cursor-install, policy-menu, policy-tip, usage, keybinding-menu) | 9 to 36 | Curved-edge coverage at popover, card and field corners over the light page; `appearance` and `cursor-install` add the page's classic thumb across a rounding boundary (as `prs/actions-merged`) |
| `settings/appearance`, `settings/cursor-install` | 22, 13 | The page's classic thumb across a rounding boundary (as `prs/actions-merged`) and the Nord card's top corners |
| `settings/compat-modal`, `settings/compat-modal-paper` | 55, 102 | Glyph-top pixels of the modal's prefilled field values and, on paper, the panel's corners |
| `home/welcome`, `home/welcome-modes` | 11 | Curved-edge coverage where the rows' rounded ends meet the list's border arc |
| `home/new-project-path` | 102 | The focused field's corners and the typed path |
| `home/new-task-slash` | 3 | Curved-edge coverage at the slash menu's corner |
| `briefing/schedule-editor`, `briefing/schedule-editor-once`, `briefing/schedule-editor-paper` | 38, 0, 50 | The cron expression and date fields |
| `projects/general`, `projects/general-tall`, `projects/general-paper` | 57, 57, 85 | The custom color swatch's 1px dashed circle: WebKit dashes a curved border with its own pattern and coverage; `paper` adds typed input text (the repository path) |
| `palettes/goto-filter`, `palettes/goto-empty`, `palettes/commands-filter`, `palettes/syntax-filter`, `palettes/shortcuts-filter` | 13, 48, 1, 2, 26 | The typed query and, in `goto-empty`, a 1px row under the input a device pixel off |
| `projects/run`, `projects/run-tall` | 18, 86 | Curved-edge coverage at the run cards' corners |
| `projects/run-resized` | 84 | `projects/run`'s, plus the focused box's amber border at its rounded corners and the page's classic thumb on the taller page (scroll height across a rounding boundary) |

## Results

Last full run (`hard`/`text`/`aa` are differing pixels at device scale 2; `tolerance` and `hard ok` are the allowances in `scenes.json`):

| Scene | Hard | Text | AA | Tolerance | Hard ok |
|---|---|---|---|---|---|
| `armed-button/default` | 0 | 0 | 436 | 480 | 0 |
| `buttons/default` | 0 | 602 | 1624 | 2450 | 0 |
| `buttons/paper` | 0 | 604 | 1584 | 2410 | 0 |
| `icon-picker/closed` | 0 | 0 | 60 | 70 | 0 |
| `icon-picker/open` | 3 | 0 | 371 | 410 | 3 |
| `icons/app` | 0 | 0 | 86 | 100 | 0 |
| `icons/file` | 0 | 0 | 0 | 0 | 0 |
| `icons/file-light` | 0 | 0 | 0 | 0 | 0 |
| `icons/lucide` | 0 | 0 | 55 | 70 | 0 |
| `icons/provider` | 0 | 0 | 0 | 0 | 0 |
| `kbd/default` | 0 | 0 | 208 | 230 | 0 |
| `overlays/misc` | 0 | 46 | 126 | 190 | 0 |
| `overlays/misc-paper` | 15 | 47 | 215 | 290 | 15 |
| `overlays/modal` | 0 | 0 | 201 | 230 | 0 |
| `overlays/palette` | 0 | 124 | 506 | 700 | 0 |
| `project-pic/default` | 0 | 0 | 351 | 390 | 0 |
| `resize-handle/focus` | 0 | 0 | 0 | 0 | 0 |
| `resize-handle/hover` | 0 | 0 | 0 | 0 | 0 |
| `ring/default` | 0 | 0 | 10 | 20 | 0 |
| `scrollbar/default` | 0 | 0 | 32 | 40 | 0 |
| `scrollbar/paper` | 0 | 0 | 40 | 50 | 0 |
| `select/closed` | 0 | 869 | 148 | 1120 | 0 |
| `select/closed-paper` | 0 | 866 | 250 | 1230 | 0 |
| `select/focus` | 7 | 0 | 210 | 240 | 7 |
| `select/multiple` | 2 | 1 | 192 | 220 | 2 |
| `select/open` | 2 | 198 | 244 | 490 | 2 |
| `select/searchable` | 2 | 176 | 280 | 510 | 2 |
| `settings-group/default` | 0 | 103 | 801 | 1000 | 0 |
| `settings-group/paper` | 0 | 103 | 958 | 1170 | 0 |
| `tab-strip/default` | 0 | 68 | 145 | 240 | 0 |
| `tab-strip/hover` | 0 | 1199 | 145 | 1480 | 0 |
| `tab-strip/overflow` | 0 | 383 | 0 | 430 | 0 |
| `tab-strip/paper` | 0 | 95 | 228 | 360 | 0 |
| `text/calibration` | 0 | 395 | 0 | 440 | 0 |
| `toasts/default` | 0 | 703 | 669 | 1510 | 0 |
| `toasts/paper` | 4 | 682 | 840 | 1680 | 4 |
| `toggles/catppuccin-latte` | 0 | 0 | 430 | 480 | 0 |
| `toggles/default` | 0 | 0 | 482 | 540 | 0 |
| `tokens/catppuccin-latte` | 0 | 0 | 0 | 0 | 0 |
| `tokens/catppuccin-mocha` | 0 | 0 | 0 | 0 | 0 |
| `tokens/coolglow` | 0 | 0 | 0 | 0 | 0 |
| `tokens/default` | 0 | 0 | 0 | 0 | 0 |
| `tokens/nord` | 0 | 0 | 0 | 0 | 0 |
| `tokens/paper` | 0 | 0 | 0 | 0 | 0 |
| `tokens/slate` | 0 | 0 | 0 | 0 | 0 |
| `tooltip/above` | 0 | 1318 | 191 | 1660 | 0 |
| `tooltip/below` | 0 | 59 | 206 | 300 | 0 |
| `tooltip/paper` | 0 | 56 | 280 | 370 | 0 |
| `editor/code` | 0 | 13445 | 0 | 14790 | 0 |
| `editor/paper` | 0 | 13033 | 0 | 14340 | 0 |
| `editor/find` | 41 | 10803 | 521 | 12460 | 41 |
| `editor/vim` | 132 | 13510 | 224 | 15110 | 132 |
| `terminal/prompt` | 0 | 0 | 6 | 10 | 0 |
| `terminal/paper` | 0 | 0 | 4 | 10 | 0 |
| `terminal/selection` | 0 | 0 | 6 | 10 | 0 |
| `terminal/search` | 0 | 0 | 312 | 350 | 0 |
| `terminal/scrollbar` | 0 | 0 | 0 | 0 | 0 |
| `terminal/decorations` | 0 | 0 | 6 | 10 | 0 |
| `terminal/sgr` | 0 | 0 | 0 | 0 | 0 |
| `terminal/glyphs` | 0 | 0 | 0 | 0 | 0 |
| `terminal/solarized-light` | 0 | 0 | 7 | 10 | 0 |
| `terminal/solarized-dark` | 0 | 0 | 8 | 10 | 0 |
| `shell/default` | 0 | 268 | 560 | 920 | 0 |
| `shell/paper` | 0 | 261 | 572 | 920 | 0 |
| `shell/nord` | 0 | 268 | 557 | 910 | 0 |
| `shell/collapsed` | 0 | 1 | 52 | 60 | 0 |
| `shell/peek` | 0 | 268 | 560 | 920 | 0 |
| `shell/groups` | 0 | 235 | 665 | 1000 | 0 |
| `shell/tree` | 1 | 407 | 662 | 1180 | 1 |
| `shell/hover` | 1 | 425 | 623 | 1160 | 1 |
| `shell/scrolled` | 2 | 376 | 444 | 910 | 2 |
| `shell/base-select` | 0 | 959 | 797 | 1940 | 0 |
| `shell/issue` | 0 | 703 | 676 | 1520 | 0 |
| `markdown/chat` | 82 | 2084 | 5532 | 8380 | 82 |
| `markdown/paper` | 116 | 2088 | 5661 | 8530 | 116 |
| `markdown/streaming` | 0 | 0 | 74 | 90 | 0 |
| `mermaid/flowchart` | 0 | 8716 | 1922 | 11710 | 0 |
| `mermaid/sequence` | 0 | 8466 | 16 | 9340 | 0 |
| `pdf/page` | 10 | 0 | 18317 | 20150 | 10 |
| `file-preview/markdown` | 0 | 947 | 99 | 1160 | 0 |
| `file-preview/svg` | 0 | 0 | 0 | 0 | 0 |
| `lightbox/image` | 0 | 10 | 14684 | 16170 | 0 |
| `chat/empty` | 0 | 133 | 132 | 300 | 0 |
| `chat/transcript` | 0 | 8390 | 524 | 9810 | 0 |
| `chat/transcript-paper` | 0 | 8479 | 1225 | 10680 | 0 |
| `chat/brief-card` | 0 | 2249 | 477 | 3000 | 0 |
| `chat/brief-card-paper` | 0 | 2150 | 633 | 3070 | 0 |
| `chat/streaming` | 0 | 845 | 475 | 1460 | 0 |
| `chat/permission` | 0 | 728 | 752 | 1630 | 0 |
| `chat/question` | 0 | 359 | 691 | 1160 | 0 |
| `chat/plan` | 0 | 641 | 307 | 1050 | 0 |
| `chat/fanout` | 0 | 667 | 336 | 1110 | 0 |
| `chat/composer` | 0 | 218 | 331 | 610 | 0 |
| `chat/mode-ask` | 0 | 0 | 132 | 150 | 0 |
| `chat/mode-plan` | 0 | 0 | 132 | 150 | 0 |
| `chat/mode-agent` | 0 | 0 | 132 | 150 | 0 |
| `chat/mode-review` | 0 | 0 | 132 | 150 | 0 |
| `chat/mode-loop` | 0 | 133 | 132 | 300 | 0 |
| `chat/mode-agent-paper` | 0 | 0 | 186 | 210 | 0 |
| `chat/pickers` | 0 | 310 | 247 | 620 | 0 |
| `chat/app` | 0 | 229 | 660 | 980 | 0 |
| `chat/app-paper` | 0 | 231 | 518 | 830 | 0 |
| `review/unified` | 0 | 4544 | 204 | 5230 | 0 |
| `review/unified-paper` | 0 | 1320 | 378 | 1870 | 0 |
| `review/split` | 0 | 5744 | 273 | 6620 | 0 |
| `review/hunk` | 0 | 5474 | 273 | 6330 | 0 |
| `review/collapsed` | 8 | 5365 | 195 | 6120 | 8 |
| `review/filelist` | 0 | 7243 | 252 | 8250 | 0 |
| `review/thread` | 0 | 3887 | 1345 | 5760 | 0 |
| `review/thread-paper` | 0 | 3289 | 1785 | 5590 | 0 |
| `review/commits` | 0 | 195 | 594 | 870 | 0 |
| `review/commits-paper` | 0 | 196 | 689 | 980 | 0 |
| `review/large` | 0 | 13823 | 58 | 15270 | 0 |
| `observatory/explorer` | 0 | 8630 | 113 | 9620 | 0 |
| `observatory/explorer-paper` | 0 | 8629 | 207 | 9720 | 0 |
| `observatory/code` | 0 | 31885 | 113 | 35200 | 0 |
| `observatory/code-paper` | 0 | 31888 | 207 | 35310 | 0 |
| `observatory/tabs` | 0 | 5279 | 50 | 5870 | 0 |
| `observatory/merge` | 0 | 39767 | 113 | 43870 | 0 |
| `observatory/review` | 0 | 2012 | 167 | 2400 | 0 |
| `observatory/terminal` | 0 | 31877 | 137 | 35220 | 0 |
| `observatory/terminal-beside` | 0 | 31760 | 113 | 35070 | 0 |
| `observatory/terminal-max` | 2 | 32285 | 2783 | 38580 | 2 |
| `observatory/quick-open` | 0 | 33333 | 114 | 36800 | 0 |
| `observatory/artifacts` | 0 | 31604 | 82 | 34860 | 0 |
| `observatory/pdf` | 257 | 2013 | 57296 | 65240 | 257 |
| `observatory/image` | 0 | 1775 | 3871 | 6220 | 0 |
| `observatory/commit` | 0 | 35778 | 166 | 39540 | 0 |
| `workspace/chat` | 0 | 35349 | 711 | 39670 | 0 |
| `workspace/chat-paper` | 0 | 35395 | 1503 | 40590 | 0 |
| `workspace/tabs` | 0 | 12563 | 718 | 14610 | 0 |
| `workspace/plan` | 0 | 32960 | 326 | 36620 | 0 |
| `workspace/findings` | 0 | 35352 | 355 | 39280 | 0 |
| `workspace/loop` | 0 | 35111 | 718 | 39420 | 0 |
| `workspace/loop-paper` | 0 | 34810 | 869 | 39250 | 0 |
| `workspace/narrow` | 22 | 20407 | 548 | 23060 | 22 |
| `prs/list` | 1 | 3349 | 2289 | 6210 | 1 |
| `prs/list-paper` | 0 | 3458 | 3349 | 7490 | 0 |
| `prs/list-wide` | 1 | 4112 | 2356 | 7120 | 1 |
| `prs/filters` | 1 | 3382 | 2126 | 6060 | 1 |
| `prs/people` | 1 | 3686 | 1990 | 6250 | 1 |
| `prs/detail-conv` | 1 | 1701 | 3203 | 5400 | 1 |
| `prs/detail-conv-paper` | 0 | 1820 | 3414 | 5760 | 0 |
| `prs/detail-files` | 1 | 6724 | 2225 | 9850 | 1 |
| `prs/detail-commits` | 1 | 1392 | 2020 | 3760 | 1 |
| `prs/detail-checks` | 1 | 1053 | 1578 | 2900 | 1 |
| `prs/actions-update` | 1 | 1620 | 3289 | 5400 | 1 |
| `prs/actions-ready` | 1 | 2048 | 3110 | 5680 | 1 |
| `prs/actions-checks` | 5 | 1711 | 3185 | 5390 | 5 |
| `prs/actions-blocked` | 1 | 2528 | 3096 | 6190 | 1 |
| `prs/actions-conflicts` | 1 | 2168 | 3106 | 5810 | 1 |
| `prs/actions-queued` | 1 | 2106 | 3127 | 5760 | 1 |
| `prs/actions-queue` | 1 | 1619 | 3215 | 5320 | 1 |
| `prs/actions-merged` | 14 | 2102 | 2988 | 5600 | 14 |
| `prs/actions-pending` | 1 | 1620 | 3375 | 5500 | 1 |
| `prs/actions-pending-paper` | 0 | 1813 | 3610 | 5970 | 0 |
| `prs/review-modal` | 1 | 2683 | 1621 | 4740 | 1 |
| `prs/review-modal-paper` | 0 | 3001 | 1604 | 5070 | 0 |
| `prs/actions-merge-menu` | 1 | 1828 | 3179 | 5510 | 1 |
| `prs/actions-confirm` | 1 | 1739 | 3199 | 5440 | 1 |
| `prs/actions-review-menu` | 5 | 1492 | 2779 | 4700 | 5 |
| `prs/actions-update-menu` | 0 | 1392 | 2836 | 4660 | 0 |
| `prs/people-detail` | 1 | 8377 | 3479 | 13050 | 1 |
| `settings/general` | 0 | 4779 | 1887 | 7340 | 0 |
| `settings/general-paper` | 2 | 4759 | 2169 | 7630 | 2 |
| `settings/appearance` | 22 | 4315 | 3315 | 8400 | 22 |
| `settings/appearance-paper` | 15 | 4168 | 3892 | 8870 | 15 |
| `settings/editor` | 0 | 3339 | 839 | 4600 | 0 |
| `settings/editor-paper` | 0 | 3141 | 1225 | 4810 | 0 |
| `settings/skills` | 0 | 2744 | 1283 | 4430 | 0 |
| `settings/skills-paper` | 0 | 2682 | 1448 | 4550 | 0 |
| `settings/data` | 0 | 2908 | 752 | 4030 | 0 |
| `settings/data-paper` | 0 | 2843 | 883 | 4100 | 0 |
| `settings/providers` | 0 | 3839 | 1707 | 6110 | 0 |
| `settings/providers-paper` | 0 | 3305 | 2004 | 5840 | 0 |
| `settings/models` | 0 | 4373 | 901 | 5810 | 0 |
| `settings/models-paper` | 0 | 4273 | 1191 | 6020 | 0 |
| `settings/loops` | 0 | 3074 | 567 | 4010 | 0 |
| `settings/loops-paper` | 0 | 2995 | 696 | 4070 | 0 |
| `settings/prs` | 0 | 17966 | 1580 | 21510 | 0 |
| `settings/prs-paper` | 0 | 17332 | 1956 | 21220 | 0 |
| `settings/integrations` | 0 | 3139 | 696 | 4220 | 0 |
| `settings/integrations-paper` | 0 | 2917 | 968 | 4280 | 0 |
| `settings/mcp` | 0 | 3089 | 1369 | 4910 | 0 |
| `settings/mcp-paper` | 10 | 3060 | 1792 | 5340 | 10 |
| `settings/commands` | 0 | 2907 | 1381 | 4720 | 0 |
| `settings/commands-paper` | 0 | 2727 | 1463 | 4610 | 0 |
| `settings/prompts` | 0 | 3598 | 2496 | 6710 | 0 |
| `settings/prompts-paper` | 0 | 3371 | 2646 | 6620 | 0 |
| `settings/grammars` | 0 | 4996 | 851 | 6440 | 0 |
| `settings/grammars-paper` | 0 | 4881 | 1023 | 6500 | 0 |
| `settings/lsp` | 0 | 3302 | 1439 | 5220 | 0 |
| `settings/lsp-paper` | 0 | 2990 | 1579 | 5030 | 0 |
| `settings/themes` | 0 | 4474 | 1355 | 6420 | 0 |
| `settings/themes-paper` | 0 | 4418 | 1499 | 6510 | 0 |
| `settings/snippets` | 0 | 5265 | 1153 | 7060 | 0 |
| `settings/snippets-paper` | 0 | 5142 | 1298 | 7090 | 0 |
| `settings/compat-modal` | 55 | 4432 | 4288 | 9600 | 55 |
| `settings/compat-modal-paper` | 102 | 7288 | 39615 | 51600 | 102 |
| `settings/cursor-install` | 13 | 3652 | 1808 | 6010 | 13 |
| `settings/cursor-install-paper` | 15 | 3381 | 7842 | 12350 | 15 |
| `settings/policy-menu` | 0 | 3074 | 609 | 4060 | 0 |
| `settings/policy-menu-paper` | 9 | 2995 | 787 | 4170 | 9 |
| `settings/policy-tip` | 0 | 5584 | 709 | 6930 | 0 |
| `settings/policy-tip-paper` | 14 | 5583 | 1388 | 7670 | 14 |
| `settings/usage` | 0 | 4767 | 2231 | 7700 | 0 |
| `settings/usage-paper` | 25 | 6195 | 16158 | 24590 | 25 |
| `settings/keybinding-menu` | 8 | 10801 | 21629 | 35680 | 8 |
| `settings/keybinding-menu-paper` | 2 | 5356 | 1868 | 7950 | 2 |
| `home/welcome` | 11 | 972 | 803 | 1960 | 11 |
| `home/welcome-paper` | 0 | 973 | 847 | 2010 | 0 |
| `home/welcome-modes` | 11 | 3325 | 1579 | 5400 | 11 |
| `home/welcome-done` | 0 | 1021 | 1919 | 3240 | 0 |
| `home/new-project` | 0 | 333 | 580 | 1010 | 0 |
| `home/new-project-paper` | 0 | 420 | 693 | 1230 | 0 |
| `home/new-project-path` | 102 | 342 | 2926 | 3600 | 102 |
| `home/new-task` | 0 | 532 | 584 | 1230 | 0 |
| `home/new-task-paper` | 0 | 564 | 686 | 1380 | 0 |
| `home/new-task-loop` | 0 | 303 | 562 | 960 | 0 |
| `home/new-task-repo` | 0 | 621 | 630 | 1380 | 0 |
| `home/new-task-branch` | 0 | 489 | 693 | 1310 | 0 |
| `home/new-task-mode` | 0 | 3665 | 679 | 4780 | 0 |
| `home/new-task-model` | 0 | 764 | 713 | 1630 | 0 |
| `home/new-task-flags` | 0 | 582 | 653 | 1360 | 0 |
| `home/new-task-loop-picker` | 0 | 399 | 619 | 1120 | 0 |
| `home/new-task-policy` | 0 | 312 | 598 | 1010 | 0 |
| `home/new-task-tip` | 0 | 1335 | 864 | 2420 | 0 |
| `home/new-task-slash` | 3 | 598 | 589 | 1310 | 3 |
| `briefing/inbox` | 0 | 12381 | 1765 | 15570 | 0 |
| `briefing/inbox-paper` | 0 | 12238 | 2107 | 15780 | 0 |
| `briefing/inbox-closed` | 0 | 2416 | 1270 | 4060 | 0 |
| `briefing/issues` | 0 | 476 | 1400 | 2070 | 0 |
| `briefing/issues-paper` | 0 | 454 | 1805 | 2490 | 0 |
| `briefing/issues-board` | 0 | 572 | 875 | 1600 | 0 |
| `briefing/schedules` | 0 | 382 | 1298 | 1850 | 0 |
| `briefing/schedules-paper` | 0 | 484 | 1176 | 1830 | 0 |
| `briefing/inbox-narrow` | 0 | 658 | 1059 | 1890 | 0 |
| `briefing/schedule-editor` | 38 | 4642 | 1795 | 7090 | 38 |
| `briefing/schedule-editor-once` | 0 | 5340 | 1240 | 7240 | 0 |
| `briefing/schedule-editor-paper` | 50 | 4842 | 1961 | 7490 | 50 |
| `projects/general` | 57 | 2804 | 3695 | 7150 | 57 |
| `projects/general-tall` | 57 | 3031 | 3959 | 7690 | 57 |
| `projects/general-paper` | 85 | 2728 | 3991 | 7400 | 85 |
| `projects/run` | 18 | 21548 | 866 | 24660 | 18 |
| `projects/run-resized` | 84 | 10741 | 1004 | 12920 | 84 |
| `projects/run-tall` | 86 | 52336 | 1729 | 59480 | 86 |
| `projects/schedules` | 0 | 2590 | 1189 | 4160 | 0 |
| `projects/prs` | 0 | 28364 | 2080 | 33490 | 0 |
| `projects/skills` | 0 | 2474 | 1025 | 3850 | 0 |
| `projects/mcp` | 0 | 2375 | 810 | 3510 | 0 |
| `projects/danger` | 0 | 2831 | 702 | 3890 | 0 |
| `palettes/goto` | 0 | 458 | 723 | 1300 | 0 |
| `palettes/goto-paper` | 0 | 491 | 965 | 1610 | 0 |
| `palettes/goto-filter` | 13 | 453 | 1048 | 1660 | 13 |
| `palettes/goto-empty` | 48 | 283 | 795 | 1190 | 48 |
| `palettes/commands` | 0 | 437 | 529 | 1070 | 0 |
| `palettes/commands-paper` | 0 | 493 | 633 | 1240 | 0 |
| `palettes/commands-filter` | 1 | 380 | 744 | 1240 | 1 |
| `palettes/open-in` | 0 | 843 | 727 | 1730 | 0 |
| `palettes/open-in-paper` | 0 | 852 | 1033 | 2080 | 0 |
| `palettes/open-in-filter` | 0 | 730 | 823 | 1710 | 0 |
| `palettes/run` | 0 | 767 | 771 | 1700 | 0 |
| `palettes/run-paper` | 0 | 781 | 1107 | 2080 | 0 |
| `palettes/run-filter` | 0 | 734 | 923 | 1830 | 0 |
| `palettes/stop` | 0 | 733 | 862 | 1760 | 0 |
| `palettes/syntax` | 0 | 282 | 544 | 910 | 0 |
| `palettes/syntax-paper` | 0 | 312 | 639 | 1050 | 0 |
| `palettes/syntax-filter` | 2 | 285 | 833 | 1230 | 2 |
| `palettes/shortcuts` | 0 | 715 | 900 | 1780 | 0 |
| `palettes/shortcuts-paper` | 0 | 778 | 1109 | 2080 | 0 |
| `palettes/shortcuts-filter` | 26 | 338 | 1428 | 1950 | 26 |

Shell tolerances: text is the titlebar and sidebar labels (glyph subpixel phase); anti-aliasing is icon, chip and ring edges and the glow, shadow and fade gradients.

Terminal tolerances: every grid difference left is synthetic-italic edge coverage (above). `terminal/search` adds the find field: curved-edge coverage at the corners of its 2px `--amber-d` ring and the query, which gpui-base's editor draws (typed input text, above).

Content tolerances: text is glyph subpixel phase in prose, code and diagram labels; anti-aliasing is image resampling (the PDF page, the lightbox, the inline chart), shape edges inside PDF pages and Mermaid strokes, and rounded fence and table corners.

Chat tolerances: text is glyph subpixel phase in transcript prose, code, chips, labels and placeholders; anti-aliasing is icon, chip and card edges and the composer's shadow. No chat scene allows hard pixels.

Review tolerances: text is glyph subpixel phase in code rows, paths, labels and note bodies; anti-aliasing is icon, chip, avatar and card edges.

Observatory tolerances: text is glyph subpixel phase in code, paths, labels, tabs and the terminal grid; anti-aliasing is icon, chip and card edges, the checkerboard and image resampling (the image pane, the PDF page and its shapes).

Workspace tolerances: text is glyph subpixel phase in the transcript, the composer, tab chips, pickers, panels and the Observatory beside them; anti-aliasing is icon, chip, card and dot edges, the composer's shadow and the running dot's glow.

PR tolerances: text is glyph subpixel phase in row titles, mono metadata, pills, tabs, the thread, diffs, buttons and the modal; anti-aliasing is icon, chip, avatar, card and dot edges, the menu and modal shadows and the status-dot glows.

Settings tolerances: text is glyph subpixel phase in the nav, titles, descriptions, chips, selects, status tables and the rail beside them; anti-aliasing is icon, toggle, card and dot edges, the theme card previews, the menu, popover and modal shadows (1 to 3 levels over a wide blur) and the scrim over the page (1 level, `settings/keybinding-menu`).

Palette tolerances: text is glyph subpixel phase in rows, keycaps, the footer, placeholders and the shell around the scrim (on the detail route also the titlebar's branch chip, which ellipsizes `recur/oauth-checkout` where WebKit fits it; `PARITY.md` follow-up); anti-aliasing is icon, tile, keycap and card edges and the palette shadow.
