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
- **Exact measured sizes.** `exact::measured` gives a custom measured element its exact size. Its width reaches Taffy as measured, ceiled to a LayoutUnit (the vendored gpui-pre's `request_measured_layout_exact_width`; GPUI ceils measured widths to device pixels, which moved every flex sibling after a text by up to half a device pixel). The measured leaf reports its height floored to the device grid and an `aspect_ratio` spacer adds the rest, from the last frame (`exact::exact_cell` / `settle`, which lays the window out again when it changes). A shrink-to-fit element reports its width floored too and adds the rest as an exact `hgap` beside the leaves (chat's `Flow`, the user bubble). A nowrap text is a column of two invisible leaves sized the same way; `TextStack` lays several blocks out at exact offsets.
- **In-tree children.** An element that places parts of itself (runs and chips of an inline flow) puts them in the layout tree as absolutely positioned children at exact offsets, not as roots at its snapped origin. Overlays attached to a trigger hang off it in the tree (`style::attached`: `top`/`bottom: 100%` plus the gap) instead of being anchored from its snapped bounds.
- **Probed baselines.** GPUI snaps a box's far edge from its unrounded position. Every expected baseline and inline-box edge gets an absolutely positioned probe leaf exactly that long; its snapped far edge is where WebKit paints. Glyphs and inline boxes paint at those positions.
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
- Draw text only through `recur_ui::text` / `span` / `Text::from_spans` / `text_stack` (never GPUI's text elements), give wrapping text that can change line count an id, and set a text's line height where CSS does (`normal` is not inherited 1.45).
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
- **Glyph subpixel phase.** WebKit positions glyphs from the box's fractional left edge; recur-ui knows only the device-pixel left edge (recovering the fraction would need about 31 probes per text), so a glyph can land in a neighboring subpixel bin. Counted as text in the diff.
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

## Hard-pixel allowances

Every other scene allows no hard pixels. These are the known minor differences; each scene's tolerance sits just above its measurement so a regression still fails.

| Scene | Hard | Cause |
|---|---|---|
| `toasts/paper` | 4 | Curved-edge coverage at plain toast corners over the shadow |
| `select/open`, `select/multiple`, `select/searchable`, `select/focus` | 2 to 7 | Curved-edge coverage of the 3px focus ring and the 2px outline at the trigger's corners |
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
| `settings/general`, `settings/usage`, `settings/keybinding-menu` | 2, 2, 10 | Field corners at a fractional height (the nav search box), and a pixel of the concurrency field's value |
| `settings/*-paper` (general, appearance, mcp, cursor-install, policy-menu, policy-tip, usage, keybinding-menu) | 9 to 36 | Curved-edge coverage at popover, card and field corners over the light page; `appearance` and `cursor-install` add the page's classic thumb across a rounding boundary (as `prs/actions-merged`) |
| `settings/appearance`, `settings/cursor-install` | 22, 13 | The page's classic thumb across a rounding boundary (as `prs/actions-merged`) and the Nord card's top corners |
| `settings/compat-modal`, `settings/compat-modal-paper` | 55, 102 | Glyph-top pixels of the modal's prefilled field values and, on paper, the panel's corners |
| `home/welcome`, `home/welcome-modes` | 11 | Curved-edge coverage where the rows' rounded ends meet the list's border arc |
| `home/new-project-path` | 102 | The focused field's corners and the typed path |
| `home/new-task-slash` | 3 | Curved-edge coverage at the slash menu's corner |
| `briefing/schedule-editor`, `briefing/schedule-editor-once`, `briefing/schedule-editor-paper` | 59, 29, 63 | The cron expression and date fields |
| `projects/general`, `projects/general-tall`, `projects/general-paper` | 57, 57, 85 | The custom color swatch's 1px dashed circle: WebKit dashes a curved border with its own pattern and coverage; `paper` adds typed input text (the repository path) |
| `palettes/goto-filter`, `palettes/goto-empty`, `palettes/commands-filter`, `palettes/syntax-filter`, `palettes/shortcuts-filter` | 13, 48, 1, 2, 26 | The typed query and, in `goto-empty`, a 1px row under the input a device pixel off |
| `projects/run`, `projects/run-tall` | 18, 86 | Curved-edge coverage at the run cards' corners |
| `projects/run-resized` | 84 | `projects/run`'s, plus the focused box's amber border at its rounded corners and the page's classic thumb on the taller page (scroll height across a rounding boundary) |

## Results

Last full run (`hard`/`text`/`aa` are differing pixels at device scale 2; `tolerance` and `hard ok` are the allowances in `scenes.json`):

| Scene | Hard | Text | AA | Tolerance | Hard ok |
|---|---|---|---|---|---|
| `armed-button/default` | 0 | 1486 | 436 | 2120 | 0 |
| `buttons/default` | 0 | 4822 | 1624 | 7100 | 0 |
| `buttons/paper` | 0 | 4849 | 1584 | 7080 | 0 |
| `icon-picker/closed` | 0 | 0 | 60 | 70 | 0 |
| `icon-picker/open` | 3 | 0 | 371 | 420 | 3 |
| `icons/app` | 0 | 0 | 86 | 100 | 0 |
| `icons/file` | 0 | 0 | 0 | 0 | 0 |
| `icons/file-light` | 0 | 0 | 0 | 0 | 0 |
| `icons/lucide` | 0 | 0 | 55 | 70 | 0 |
| `icons/provider` | 0 | 0 | 0 | 0 | 0 |
| `kbd/default` | 0 | 1288 | 208 | 1650 | 0 |
| `overlays/misc` | 0 | 2865 | 126 | 3300 | 0 |
| `overlays/misc-paper` | 15 | 2945 | 215 | 3490 | 15 |
| `overlays/modal` | 0 | 0 | 201 | 230 | 0 |
| `overlays/palette` | 0 | 1259 | 506 | 1950 | 0 |
| `project-pic/default` | 0 | 165 | 351 | 570 | 0 |
| `resize-handle/focus` | 0 | 0 | 0 | 0 | 0 |
| `resize-handle/hover` | 0 | 0 | 0 | 0 | 0 |
| `ring/default` | 0 | 362 | 10 | 410 | 0 |
| `scrollbar/default` | 0 | 0 | 32 | 40 | 0 |
| `scrollbar/paper` | 0 | 0 | 40 | 50 | 0 |
| `select/closed` | 0 | 1455 | 148 | 1770 | 0 |
| `select/closed-paper` | 0 | 1508 | 250 | 1940 | 0 |
| `select/focus` | 7 | 0 | 210 | 240 | 7 |
| `select/multiple` | 2 | 1 | 192 | 240 | 2 |
| `select/open` | 2 | 1167 | 244 | 1570 | 2 |
| `select/searchable` | 2 | 1144 | 280 | 1580 | 2 |
| `settings-group/default` | 0 | 103 | 801 | 1000 | 0 |
| `settings-group/paper` | 0 | 103 | 958 | 1170 | 0 |
| `tab-strip/default` | 0 | 2309 | 145 | 2700 | 0 |
| `tab-strip/hover` | 0 | 2918 | 145 | 3370 | 0 |
| `tab-strip/overflow` | 0 | 1131 | 0 | 2520 | 0 |
| `tab-strip/paper` | 0 | 2390 | 228 | 2880 | 0 |
| `text/calibration` | 0 | 289 | 0 | 320 | 0 |
| `toasts/default` | 0 | 1674 | 669 | 2590 | 0 |
| `toasts/paper` | 4 | 1669 | 840 | 3620 | 4 |
| `toggles/catppuccin-latte` | 0 | 0 | 430 | 480 | 0 |
| `toggles/default` | 0 | 0 | 482 | 540 | 0 |
| `tokens/catppuccin-latte` | 0 | 0 | 0 | 0 | 0 |
| `tokens/catppuccin-mocha` | 0 | 0 | 0 | 0 | 0 |
| `tokens/coolglow` | 0 | 0 | 0 | 0 | 0 |
| `tokens/default` | 0 | 0 | 0 | 0 | 0 |
| `tokens/nord` | 0 | 0 | 0 | 0 | 0 |
| `tokens/paper` | 0 | 0 | 0 | 0 | 0 |
| `tokens/slate` | 0 | 0 | 0 | 0 | 0 |
| `tooltip/above` | 0 | 2449 | 191 | 2930 | 0 |
| `tooltip/below` | 0 | 3 | 206 | 240 | 0 |
| `tooltip/paper` | 0 | 0 | 280 | 480 | 0 |
| `editor/code` | 0 | 13445 | 0 | 14790 | 0 |
| `editor/paper` | 0 | 13033 | 0 | 14340 | 0 |
| `editor/find` | 41 | 11525 | 521 | 13170 | 41 |
| `editor/vim` | 132 | 13510 | 224 | 15110 | 132 |
| `terminal/prompt` | 0 | 0 | 6 | 10 | 0 |
| `terminal/paper` | 0 | 0 | 4 | 10 | 0 |
| `terminal/selection` | 0 | 0 | 6 | 10 | 0 |
| `terminal/search` | 0 | 0 | 312 | 350 | 0 |
| `terminal/scrollbar` | 0 | 0 | 0 | 0 | 0 |
| `terminal/decorations` | 0 | 0 | 6 | 10 | 0 |
| `terminal/sgr` | 0 | 0 | 0 | 10 | 0 |
| `terminal/glyphs` | 0 | 0 | 0 | 10 | 0 |
| `terminal/solarized-light` | 0 | 0 | 7 | 10 | 0 |
| `terminal/solarized-dark` | 0 | 0 | 8 | 10 | 0 |
| `shell/default` | 0 | 1942 | 560 | 3020 | 0 |
| `shell/paper` | 0 | 1990 | 572 | 3020 | 0 |
| `shell/nord` | 0 | 1930 | 557 | 2990 | 0 |
| `shell/collapsed` | 0 | 598 | 52 | 720 | 0 |
| `shell/peek` | 0 | 1942 | 560 | 3020 | 0 |
| `shell/groups` | 0 | 1842 | 665 | 3020 | 0 |
| `shell/tree` | 1 | 2842 | 662 | 4210 | 1 |
| `shell/hover` | 1 | 2667 | 623 | 3880 | 1 |
| `shell/scrolled` | 2 | 2004 | 444 | 2960 | 2 |
| `shell/base-select` | 0 | 10257 | 797 | 12590 | 0 |
| `shell/issue` | 0 | 4859 | 676 | 6420 | 0 |
| `markdown/chat` | 82 | 5772 | 5494 | 12590 | 82 |
| `markdown/paper` | 116 | 5847 | 5625 | 12790 | 116 |
| `markdown/streaming` | 0 | 1875 | 74 | 2150 | 0 |
| `mermaid/flowchart` | 0 | 9173 | 1922 | 12210 | 0 |
| `mermaid/sequence` | 0 | 8076 | 16 | 8910 | 0 |
| `pdf/page` | 10 | 175 | 18331 | 20390 | 10 |
| `file-preview/markdown` | 0 | 4877 | 99 | 5470 | 0 |
| `file-preview/svg` | 0 | 0 | 0 | 0 | 0 |
| `lightbox/image` | 0 | 10 | 14684 | 16180 | 0 |
| `chat/empty` | 0 | 133 | 132 | 290 | 0 |
| `chat/transcript` | 0 | 13388 | 524 | 15310 | 0 |
| `chat/transcript-paper` | 0 | 13612 | 1225 | 16350 | 0 |
| `chat/brief-card` | 0 | 4139 | 477 | 5080 | 0 |
| `chat/brief-card-paper` | 0 | 4034 | 633 | 5150 | 0 |
| `chat/streaming` | 0 | 1270 | 456 | 1900 | 0 |
| `chat/permission` | 0 | 2689 | 752 | 4900 | 0 |
| `chat/question` | 0 | 1092 | 691 | 1970 | 0 |
| `chat/plan` | 0 | 1297 | 307 | 1770 | 0 |
| `chat/fanout` | 0 | 4415 | 336 | 5230 | 0 |
| `chat/composer` | 0 | 2276 | 331 | 2950 | 0 |
| `chat/mode-ask` | 0 | 978 | 132 | 1300 | 0 |
| `chat/mode-plan` | 0 | 340 | 132 | 520 | 0 |
| `chat/mode-agent` | 0 | 775 | 132 | 1000 | 0 |
| `chat/mode-review` | 0 | 437 | 132 | 630 | 0 |
| `chat/mode-loop` | 0 | 486 | 132 | 680 | 0 |
| `chat/mode-agent-paper` | 0 | 787 | 186 | 1090 | 0 |
| `chat/pickers` | 0 | 3889 | 247 | 4580 | 0 |
| `chat/app` | 0 | 1866 | 660 | 2780 | 0 |
| `chat/app-paper` | 0 | 1884 | 518 | 2660 | 0 |
| `review/unified` | 0 | 7372 | 204 | 8450 | 0 |
| `review/unified-paper` | 0 | 4140 | 378 | 5120 | 0 |
| `review/split` | 0 | 8690 | 273 | 9990 | 0 |
| `review/hunk` | 0 | 8420 | 273 | 9670 | 0 |
| `review/collapsed` | 8 | 8192 | 195 | 9300 | 8 |
| `review/filelist` | 0 | 9491 | 252 | 10780 | 0 |
| `review/thread` | 0 | 5122 | 1345 | 7490 | 0 |
| `review/thread-paper` | 0 | 4522 | 1785 | 7320 | 0 |
| `review/commits` | 0 | 3014 | 594 | 3970 | 0 |
| `review/commits-paper` | 0 | 3023 | 689 | 4090 | 0 |
| `review/large` | 0 | 22570 | 58 | 24860 | 0 |
| `observatory/explorer` | 0 | 10859 | 113 | 12210 | 0 |
| `observatory/explorer-paper` | 0 | 10887 | 207 | 12360 | 0 |
| `observatory/code` | 0 | 34505 | 113 | 36790 | 0 |
| `observatory/code-paper` | 0 | 34608 | 207 | 37020 | 0 |
| `observatory/tabs` | 0 | 8116 | 50 | 9020 | 0 |
| `observatory/merge` | 0 | 41467 | 113 | 44100 | 0 |
| `observatory/review` | 0 | 3711 | 167 | 4160 | 0 |
| `observatory/terminal` | 0 | 34497 | 137 | 36810 | 0 |
| `observatory/terminal-beside` | 0 | 34380 | 113 | 36660 | 0 |
| `observatory/terminal-max` | 2 | 33721 | 2783 | 38720 | 2 |
| `observatory/quick-open` | 0 | 35978 | 114 | 38320 | 0 |
| `observatory/artifacts` | 0 | 33062 | 82 | 35220 | 0 |
| `observatory/pdf` | 257 | 4976 | 57310 | 65870 | 257 |
| `observatory/image` | 0 | 4701 | 3871 | 9440 | 0 |
| `observatory/commit` | 0 | 37390 | 160 | 39850 | 0 |
| `workspace/chat` | 0 | 42464 | 711 | 48370 | 0 |
| `workspace/chat-paper` | 0 | 42689 | 1503 | 49560 | 0 |
| `workspace/tabs` | 0 | 20059 | 718 | 24770 | 0 |
| `workspace/plan` | 0 | 40332 | 326 | 45880 | 0 |
| `workspace/findings` | 0 | 43250 | 355 | 48850 | 0 |
| `workspace/loop` | 0 | 45002 | 718 | 51450 | 0 |
| `workspace/loop-paper` | 0 | 45210 | 869 | 51780 | 0 |
| `workspace/narrow` | 22 | 24002 | 548 | 30000 | 22 |
| `prs/list` | 1 | 20195 | 2289 | 27460 | 1 |
| `prs/list-paper` | 0 | 20428 | 3349 | 27680 | 0 |
| `prs/list-wide` | 1 | 25433 | 2356 | 34030 | 1 |
| `prs/filters` | 1 | 25932 | 2126 | 32600 | 1 |
| `prs/people` | 1 | 19263 | 1990 | 26150 | 1 |
| `prs/detail-conv` | 1 | 9740 | 3203 | 14520 | 1 |
| `prs/detail-conv-paper` | 0 | 9850 | 3414 | 14810 | 0 |
| `prs/detail-files` | 1 | 16692 | 2225 | 21170 | 1 |
| `prs/detail-commits` | 1 | 10476 | 2020 | 14030 | 1 |
| `prs/detail-checks` | 1 | 7367 | 1578 | 10120 | 1 |
| `prs/actions-update` | 1 | 12313 | 3289 | 17450 | 1 |
| `prs/actions-ready` | 1 | 8625 | 3110 | 13180 | 1 |
| `prs/actions-checks` | 5 | 10109 | 3185 | 14910 | 5 |
| `prs/actions-blocked` | 1 | 11840 | 3096 | 16710 | 1 |
| `prs/actions-conflicts` | 1 | 11951 | 3106 | 16850 | 1 |
| `prs/actions-queued` | 1 | 10175 | 3127 | 14920 | 1 |
| `prs/actions-queue` | 1 | 11251 | 3215 | 16200 | 1 |
| `prs/actions-merged` | 14 | 8078 | 2988 | 12440 | 14 |
| `prs/actions-pending` | 1 | 11371 | 3375 | 16500 | 1 |
| `prs/actions-pending-paper` | 0 | 11406 | 3610 | 16740 | 0 |
| `prs/review-modal` | 1 | 12306 | 1621 | 15710 | 1 |
| `prs/review-modal-paper` | 0 | 11910 | 1604 | 18940 | 0 |
| `prs/actions-merge-menu` | 1 | 8866 | 3179 | 13530 | 1 |
| `prs/actions-confirm` | 1 | 10845 | 3199 | 15730 | 1 |
| `prs/actions-review-menu` | 5 | 9460 | 2779 | 13730 | 5 |
| `prs/actions-update-menu` | 0 | 12537 | 2836 | 17210 | 0 |
| `prs/people-detail` | 1 | 16128 | 3479 | 21850 | 1 |
| `settings/general` | 2 | 5856 | 1946 | 7850 | 2 |
| `settings/general-paper` | 13 | 5846 | 2217 | 8070 | 13 |
| `settings/appearance` | 22 | 9831 | 3315 | 13390 | 22 |
| `settings/appearance-paper` | 15 | 9713 | 3892 | 13790 | 15 |
| `settings/editor` | 0 | 7729 | 839 | 8810 | 0 |
| `settings/editor-paper` | 0 | 7524 | 1225 | 8940 | 0 |
| `settings/skills` | 0 | 3821 | 1283 | 5350 | 0 |
| `settings/skills-paper` | 0 | 3769 | 1448 | 5400 | 0 |
| `settings/data` | 0 | 5210 | 752 | 6200 | 0 |
| `settings/data-paper` | 0 | 5181 | 883 | 6250 | 0 |
| `settings/providers` | 0 | 6542 | 1707 | 8490 | 0 |
| `settings/providers-paper` | 0 | 6043 | 2004 | 8230 | 0 |
| `settings/models` | 0 | 8842 | 901 | 9980 | 0 |
| `settings/models-paper` | 0 | 8774 | 1191 | 10150 | 0 |
| `settings/loops` | 0 | 4499 | 567 | 5310 | 0 |
| `settings/loops-paper` | 0 | 4437 | 696 | 5320 | 0 |
| `settings/prs` | 0 | 19043 | 1580 | 20860 | 0 |
| `settings/prs-paper` | 0 | 18419 | 1956 | 20560 | 0 |
| `settings/integrations` | 0 | 5051 | 696 | 5990 | 0 |
| `settings/integrations-paper` | 0 | 4855 | 968 | 6010 | 0 |
| `settings/mcp` | 0 | 5669 | 1369 | 7280 | 0 |
| `settings/mcp-paper` | 10 | 5619 | 1792 | 7600 | 10 |
| `settings/commands` | 0 | 4192 | 1381 | 5810 | 0 |
| `settings/commands-paper` | 0 | 4023 | 1463 | 5670 | 0 |
| `settings/prompts` | 0 | 4675 | 2496 | 7410 | 0 |
| `settings/prompts-paper` | 0 | 4458 | 2646 | 7290 | 0 |
| `settings/grammars` | 0 | 6073 | 851 | 7170 | 0 |
| `settings/grammars-paper` | 0 | 5968 | 1023 | 7180 | 0 |
| `settings/lsp` | 0 | 5256 | 1439 | 6940 | 0 |
| `settings/lsp-paper` | 0 | 4950 | 1579 | 6720 | 0 |
| `settings/themes` | 0 | 5551 | 1355 | 7150 | 0 |
| `settings/themes-paper` | 0 | 5505 | 1499 | 7190 | 0 |
| `settings/snippets` | 0 | 6545 | 1153 | 7940 | 0 |
| `settings/snippets-paper` | 0 | 6431 | 1298 | 7920 | 0 |
| `settings/compat-modal` | 55 | 7661 | 4288 | 12100 | 55 |
| `settings/compat-modal-paper` | 102 | 10590 | 39615 | 58320 | 102 |
| `settings/cursor-install` | 13 | 7203 | 1808 | 9250 | 13 |
| `settings/cursor-install-paper` | 15 | 6945 | 7842 | 14970 | 15 |
| `settings/policy-menu` | 0 | 9038 | 609 | 9890 | 0 |
| `settings/policy-menu-paper` | 9 | 9038 | 787 | 10040 | 9 |
| `settings/policy-tip` | 0 | 7234 | 709 | 8190 | 0 |
| `settings/policy-tip-paper` | 14 | 7258 | 1388 | 8730 | 14 |
| `settings/usage` | 2 | 5844 | 2290 | 8270 | 2 |
| `settings/usage-paper` | 36 | 7282 | 16206 | 23600 | 36 |
| `settings/keybinding-menu` | 10 | 12079 | 21691 | 34190 | 10 |
| `settings/keybinding-menu-paper` | 13 | 6649 | 1920 | 13800 | 13 |
| `home/welcome` | 11 | 2344 | 803 | 3470 | 11 |
| `home/welcome-paper` | 0 | 2357 | 847 | 3530 | 0 |
| `home/welcome-modes` | 11 | 5679 | 1579 | 7990 | 11 |
| `home/welcome-done` | 0 | 4702 | 1919 | 7550 | 0 |
| `home/new-project` | 0 | 3038 | 580 | 4040 | 0 |
| `home/new-project-paper` | 0 | 3176 | 693 | 4350 | 0 |
| `home/new-project-path` | 102 | 3100 | 2926 | 7370 | 102 |
| `home/new-task` | 0 | 4052 | 584 | 5160 | 0 |
| `home/new-task-paper` | 0 | 4121 | 686 | 5380 | 0 |
| `home/new-task-loop` | 0 | 3109 | 562 | 4100 | 0 |
| `home/new-task-repo` | 0 | 4141 | 630 | 5330 | 0 |
| `home/new-task-branch` | 0 | 10016 | 693 | 11890 | 0 |
| `home/new-task-mode` | 0 | 9751 | 679 | 11540 | 0 |
| `home/new-task-model` | 0 | 6664 | 713 | 8190 | 0 |
| `home/new-task-flags` | 0 | 6856 | 653 | 8320 | 0 |
| `home/new-task-loop-picker` | 0 | 3232 | 619 | 4350 | 0 |
| `home/new-task-policy` | 0 | 6301 | 598 | 7710 | 0 |
| `home/new-task-tip` | 0 | 8124 | 864 | 9960 | 0 |
| `home/new-task-slash` | 3 | 9024 | 589 | 11330 | 3 |
| `briefing/inbox` | 163 | 18260 | 1934 | 22480 | 163 |
| `briefing/inbox-paper` | 151 | 18240 | 2248 | 22760 | 151 |
| `briefing/inbox-closed` | 163 | 6389 | 1439 | 8880 | 163 |
| `briefing/issues` | 0 | 18605 | 1400 | 23620 | 0 |
| `briefing/issues-paper` | 0 | 18862 | 1805 | 23360 | 0 |
| `briefing/issues-board` | 0 | 9986 | 875 | 12210 | 0 |
| `briefing/schedules` | 20 | 5492 | 1426 | 8000 | 20 |
| `briefing/schedules-paper` | 22 | 5679 | 1326 | 8040 | 22 |
| `briefing/inbox-narrow` | 0 | 3969 | 1059 | 5800 | 0 |
| `briefing/schedule-editor` | 81 | 11067 | 2000 | 14620 | 81 |
| `briefing/schedule-editor-once` | 51 | 10402 | 2269 | 13470 | 51 |
| `briefing/schedule-editor-paper` | 85 | 11473 | 2197 | 16160 | 85 |
| `projects/general` | 57 | 4796 | 3704 | 8940 | 57 |
| `projects/general-tall` | 57 | 5023 | 3968 | 9480 | 57 |
| `projects/general-paper` | 85 | 4744 | 4004 | 9170 | 85 |
| `projects/run` | 185 | 22799 | 1931 | 27550 | 185 |
| `projects/run-resized` | 251 | 11992 | 2069 | 27550 | 251 |
| `projects/run-tall` | 253 | 53587 | 2794 | 62300 | 253 |
| `projects/schedules` | 0 | 4790 | 1189 | 6840 | 0 |
| `projects/prs` | 0 | 29441 | 2080 | 34940 | 0 |
| `projects/skills` | 0 | 3551 | 1025 | 5300 | 0 |
| `projects/mcp` | 0 | 3452 | 810 | 4950 | 0 |
| `projects/danger` | 0 | 4948 | 702 | 6480 | 0 |
| `palettes/goto` | 0 | 3964 | 723 | 5220 | 0 |
| `palettes/goto-paper` | 0 | 4100 | 965 | 5670 | 0 |
| `palettes/goto-filter` | 13 | 2824 | 1048 | 4570 | 13 |
| `palettes/goto-empty` | 48 | 2654 | 795 | 4080 | 48 |
| `palettes/commands` | 0 | 2506 | 529 | 3400 | 0 |
| `palettes/commands-paper` | 0 | 2625 | 633 | 3680 | 0 |
| `palettes/commands-filter` | 1 | 2342 | 744 | 3320 | 1 |
| `palettes/open-in` | 0 | 6446 | 727 | 8060 | 0 |
| `palettes/open-in-paper` | 0 | 6560 | 1033 | 8540 | 0 |
| `palettes/open-in-filter` | 0 | 6333 | 823 | 7960 | 0 |
| `palettes/run` | 0 | 11609 | 771 | 13780 | 0 |
| `palettes/run-paper` | 0 | 11848 | 1107 | 14430 | 0 |
| `palettes/run-filter` | 0 | 8468 | 923 | 10450 | 0 |
| `palettes/stop` | 0 | 8280 | 862 | 9600 | 0 |
| `palettes/syntax` | 0 | 3429 | 544 | 4430 | 0 |
| `palettes/syntax-paper` | 0 | 3511 | 639 | 4660 | 0 |
| `palettes/syntax-filter` | 2 | 2790 | 833 | 3970 | 2 |
| `palettes/shortcuts` | 0 | 4392 | 900 | 5880 | 0 |
| `palettes/shortcuts-paper` | 0 | 4557 | 1109 | 6330 | 0 |
| `palettes/shortcuts-filter` | 26 | 3090 | 1428 | 5340 | 26 |

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
