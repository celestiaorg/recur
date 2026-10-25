# Recur in Rust

This is the native Rust/GPUI port of Recur. It lives in `rust/` and runs side by side with the Wails app (Go backend in `internal/`, Svelte UI in `frontend/`) until it reaches full parity. Nothing outside `rust/` is changed by the port, except Taskfile and CI entries that build it.

The goal is a faithful port: every feature, behavior, persisted format, keyboard path and pixel of the current app carries over. `docs/PARITY.md` is the checklist. An area is done when its items are ticked, its Go and frontend tests are ported, and its screens match (see `docs/ui-parity.md`).

## Layout

```
rust/
  Cargo.toml            workspace, shared dependency versions, lints
  crates/
    domain/             recur-domain       pure model + rules        (internal/domain, internal/shortid)
    store/              recur-store        SQLite, migrations, paths (internal/store)
    git/                recur-git          local git                 (internal/git)
    chroma/             recur-chroma       syntax highlighting       (internal/git highlight.go, chroma)
    project/            recur-project      project.toml, env, runs   (internal/projfile, wsenv, runconfig)
    forge/              recur-forge        remote hosting: GitHub    (engine github*, pr*, app/gh_image)
    issues/             recur-issues       issue trackers: Linear    (app/issues, Linear bundle glue)
    agent/              recur-agent        Provider trait, events    (engine provider/runner/permission/streamjson contracts)
    agent-tools/        recur-agent-tools  host tools for agents     (internal/agenttools)
    mcp/                recur-mcp          MCP host server           (internal/mcp)
    bundles/            recur-bundles      Recur Bundles             (internal/bundles)
    textmate/           recur-textmate     grammars + themes         (frontend lib/tm-*, internal/bundles/grammar)
    lsp/                recur-lsp          language server client    (internal/lsp, frontend lib/lsp.ts)
    term/               recur-term         PTY + emulator state      (internal/term)
    code/               recur-code         worktree files and search (internal/code, rg, copyfiles)
    usage/              recur-usage        provider usage polling    (internal/usage)
    engine/             recur-engine       orchestration             (internal/engine minus providers/forge)
    platform/           recur-platform     OS integration            (power, osfonts, cliinstall, openipc, instance, notifications, updater)
    services/           recur-services     UI-facing API + events    (internal/app)
    ui/                 recur-ui           design system on gpui-kit (frontend styles, shared components)
    editor/             recur-editor       code + merge editor       (CodeEditor, MergeEditor, cm-*)
    terminal-view/      recur-terminal-view terminal rendering        (TerminalPane, xterm)
    content/            recur-content      rich content              (MarkdownHtml, StreamingMarkdown, markdown*, sanitize-lsp, Mermaid, PDF, FilePreview, lightbox)
    views/
      core/             recur-view-core    shared app state, routing (lib/stores.ts, api.ts, commands.ts, workspace-nav.ts, …)
      sidebar/          recur-view-sidebar the app sidebar           (Sidebar.svelte, ProviderUsageMeter.svelte)
      <area>/           recur-view-<area>  one crate per view area   (views/*.svelte and the components only they use)
    app/                recur-app          the window shell          (App.svelte: titlebar, routing, overlays, menu, keys)
    desktop/            bin `recur`        desktop entry point       (main.go, open_boot.go)
    cli/                bin `recur-cli`    command line              (cmd/recur, internal/cli)
  providers/
    claude/ codex/ cursor/ compatible/     one crate per agent provider
  docs/
```

Remaining Go packages map as follows: `observe` and `simplify` live as modules of `recur-engine` (or `recur-code` for pure file logic) unless their consumers show a better home; record the final home in `PARITY.md`.

Views live in their own crates under `crates/views/<area>` (see "View crates" below). The workspace `members` list `crates/[!v]*` and `crates/views/*`: `crates/views` itself has no manifest, so a plain `crates/*` glob would fail.

## Layering

Dependencies point down only:

```
desktop, cli
app ──────────────► views/*, editor, terminal-view, ui
views/* ──────────► views/core, editor, terminal-view, content, ui, services, domain
content ──────────► editor, textmate, ui
views/core ───────► ui, editor, services, domain
services ─────────► engine, providers/*, forge, issues, usage, lsp, platform
engine ───────────► agent, agent-tools, mcp, bundles, code, git, store, term
agent, forge, issues, bundles, git, code, store, project ─► domain
textmate, term, ui ─ standalone
```

Rules:

- `recur-domain` has no I/O and no async.
- Nothing below `recur-services` knows about GPUI. Only `recur-app`, the view crates and the widget crates (`recur-ui`, `recur-editor`, `recur-terminal-view`, `recur-content`) render.
- View crates never depend on each other or on `recur-app`. What two views share (state, navigation, pure rules) lives in `recur-view-core`; what they show in common lives in `recur-ui`.
- `recur-engine` depends on the `recur-agent::Provider`, `recur-forge::Forge` and `recur-issues::IssueTracker` traits, never on a concrete implementation. `recur-services` is the composition root that registers the shipped providers, forges and trackers.

## Extension points

Adding a provider, forge or issue tracker means adding a crate (providers) or a module (forges, trackers) and one registration line in `recur-services`:

- **Agent providers** implement `recur_agent::Provider` (capabilities, model catalog, flags, start/resume/steer/cancel, normalized event stream, usage). The engine only talks to the trait.
- **Remote git hosting** implements `recur_forge::Forge` (detect from remote URL, list/sync PRs, reviews, threads, reactions, checks, merge, branch rules, mentions, images). GitHub (via `gh`) ships first.
- **Issue trackers** implement `recur_issues::IssueTracker` (search, fetch, expand to task prompt, workspace linking).
- **Bundles** keep their existing on-disk format and semantics; `recur-bundles` is a faithful port.

Registries are keyed by the same string ids the Go app persists, so stored data stays valid: providers `claude-code`, `codex`, `cursor` and each compatible provider's own id; forge `github` (PR rows keep `source = "gh"`); trackers `github` and `linear`. See `docs/engine-port-plan.md` section 2. The engine contains no provider, forge or tracker id string compares outside registry lookup.

The engine port is planned slice by slice in `docs/engine-port-plan.md`.

## Editor

`recur-editor` ports Recur's CodeMirror 6 editor (`CodeEditor.svelte`, `MergeEditor.svelte`, `lib/cm-*`) and `recur-textmate` its TextMate stack (`vscode-textmate` semantics on Oniguruma, `tm-*`, `codehl`). `recur-textmate` stands alone: grammars and themes reach it as loaders and parsed JSON, never through `recur-bundles`.

**Architecture.** The editor is not built on gpui-base's text input. It ports CodeMirror's model: an immutable `EditorState` over a rope `Text` (UTF-8 offsets; LSP positions convert to UTF-16), `ChangeSet` / `Transaction` / `Effect`, the selection, history, search, autocomplete, close-brackets, snippet, fold, lint and merge state, and the keymap with its precedence. `EditorView` is a GPUI entity that applies transactions and lays rows out (`display.rs`, wrapping and folds measured through the GPUI text system). One custom `EditorElement` paints in CodeMirror's layer order (scroller fill, selection, line and mark backgrounds, text, cursors, gutters, scrollbars), visible rows only, so large files stay cheap. Chrome around it (panels, tooltips, documentation, the vim panel, gutter icons) is ordinary `recur-ui` elements. vim and emacs are full ports of `@replit/codemirror-vim` 6.3.0 and `@replit/codemirror-emacs` 6.1.0 that run after the editor keymap, as Recur's keymap compartment orders them.

**Public API.**

- `EditorView::new(EditorOptions, window, cx)`: `kind` (`Code`, or `Merge { original }` for the Diff view), `path`, `doc`, `saved_doc`, `read_only`, `prefs: EditorPrefs`, `theme: Option<Rc<ParsedTmTheme>>` with `dark_fallback`, `textmate: Option<SharedTextMate>`, `pane`, `snippets: Option<Rc<dyn SnippetProvider>>`, `view_state`, `lit`.
- Host calls: `get_doc` / `peek_doc`, `is_dirty`, `mark_saved`, `apply_saved_baseline`, `replace_doc`, `external_doc_change`, `set_prefs`, `set_theme`, `set_read_only`, `set_lit`, `set_findings`, `set_diff_base`, `set_diagnostics`, `get_view_state` / `apply_view_state`, `scroll_to_line`, `toggle_search`, `apply_snippet_at`, `apply_output`, `mount_path` / `retain_paths` / `forget_path` (multi-buffer views), `coords_at_pos`, `title_at`.
- `EditorEvent`: `DocChanged`, `DirtyChanged`, `Save`, `CopyLine`, `Cursor`, `GotoDefinition`, `SearchOpenChanged`, `OpenFinding`, `PickSnippet`, `Announce`, `Toast`.
- Session and view state: `session` (the `editor-session.ts` shape, caps, wire format and patch gate) and `view_state` (caret, scroll, view mode, dirty rules).
- LSP: the host attaches an `Rc<dyn LspClient>` with `set_lsp(client, uri, language_id)` and pushes diagnostics with `set_lsp_diagnostics`. `LspClient` is the transport-facing half of `@codemirror/lsp-client`: capabilities, `request` (id + response future) and `cancel_request`, `sync`, `open_file` / `close_file` / `file_changed`, `display_file` for cross-file jumps and `apply_workspace_edit`. `lsp::content_changes_for` builds `didChange` bodies.
- Theme inputs: chrome tokens come from the `recur_ui::Theme` global (followed as it changes); code colors from the parsed TextMate theme, which the view makes the catalog's active theme (`TextMate::activate_theme`).

**What hosts provide.** `recur_ui::init` and `recur_ui::Assets`, the view inside `style::window_root` with a `TitleTooltip` as the last child (panel and gutter titles use `recur_ui::tooltip::titled`); a `SharedTextMate` with every grammar registered (`gallery::shipped_textmate` shows the conversion from `recur-bundles`) and the editor theme parsed; a `SnippetProvider` over bundle snippets; handling of `EditorEvent`s (save, toasts, finding and definition navigation); and an `LspClient` adapter over `recur_lsp::Manager` that owns request ids, `initialize`, document versions and `didChange` batching, applying `recur_lsp::protocol` around `Manager::write`.

## Rich content

`recur-content` is a widget crate, not a view crate: the chat, PR, artifact and file views embed its entities and own their state.

**Markdown.** `marked` 18 (GFM, breaks) and DOMPurify 3 are ported verbatim (`marked/`, `purify/`, JS regex semantics through `regress`, html5ever with scripting off as `DOMParser`), with goldens generated from the JS libraries (`tests/golden/`). `markdown::render_markdown` is `renderMarkdown` (Recur's renderer, emoji, codehl fences, Mermaid placeholders, GitHub image proxying, the `afterSanitizeAttributes` hook); `MarkdownHost` is `markdown-host.ts` (byte-budget cache, worker threads, 2s watchdog). The sanitized HTML is laid out natively (`render/`): the `.chat-canvas` / `.md-doc` stylesheets as data, CSS margin collapsing, WebKit's list markers and table layout, text through `recur_ui::text`, and a mirror of its line layout for link hits and find rects. No HTML from an agent or a PR ever reaches a webview; images load only from `data:` and through the host's `ImageLoader` (the `/gh-image` proxy).

**Embedding.**

- `MarkdownView::new(text, MarkdownClass, demote_images, cx)` (`MarkdownHtml`), `set_text`; events `MarkdownEvent::{LightboxOpen, Toast, TextChanged}`. `StreamingMarkdown::new(cx)`, `set(text, active, complete, cx)`, `set_document_visible`.
- `FilePreview::new(kind, content, path, SharedScroll, window, cx)`: `set_content`, `set_path`, `set_active`, `set_parked`, `toggle_search` / `open_search` / `close_search`; `FilePreviewEvent::SearchOpenChanged`. The view keeps one `SharedScroll` (`PreviewScrollStore`) per workspace, as `createPreviewScrollStore` was.
- `MermaidPreview`, `PdfPreview::new(Option<Arc<[u8]>>, window, cx)` / `set_data`, `HtmlPreview`, `AttachmentLightbox::new(src, name, image, on_close, window, cx)`.
- Hosts call `set_image_loader` once (GitHub images through the forge), turn `Toast`s into toasts and `LightboxOpen` into `AppState::set_chat_occluder`.
- `sanitize_lsp` is `lib/sanitize-lsp.ts` for HTML-string consumers. recur-editor does not need it: it lays LSP documentation out from pulldown-cmark events and drops raw HTML other than `<br>`.

**Mermaid: mermaid.js in a hidden WebKit page, drawn natively.** Mermaid lays diagrams out from live DOM text measurement, so only WebKit measuring with the app's fonts reproduces its geometry; a native layout would be a second implementation of every diagram type. `mermaid/engine.rs` runs the vendored mermaid 11.16.1 (`assets/mermaid`, copied from `frontend/node_modules` by `fetch.sh`) in a never-shown WKWebView (`webhost.rs`: non-persistent store, CSP `default-src 'none'`, `callAsyncJavaScript`) at the window's device scale, since mermaid's measurements change with it. The returned SVG is sanitized with the Svelte config, WebKit inlines its resolved paint, and labels are measured there (start positions, advances, fonts). The geometry rasterizes through CoreGraphics (resvg for filters, masks and patterns); labels are `recur_ui::text` at WebKit's positions, so find highlights and selection work like any other text. The alternative, rendering the SVG to a bitmap in the webview, would lose native text. Non-macOS builds report that Mermaid needs WebKit until WebKitGTK is linked.

**PDF: pdf.js 6 in a hidden WebKit page.** The preview's output is pdf.js's canvas, so running pdf.js 6.3.289 (vendored under `assets/pdfjs` with its `cmaps`, `standard_fonts` and `wasm`) in WebKit is the only way to match it pixel for pixel. Native rasterizers were considered and rejected: pdfium-render (Chromium's PDFium, a dylib to ship per platform) and hayro (pure Rust) are independent implementations with their own font rasterization and anti-aliasing, so neither can reproduce pdf.js's canvas, and the web preview's behavior (worker, standard-font substitution, cancellation) would have to be re-derived around them. `pdf/engine.rs` serves the library, its assets and the open documents from a `recur-pdf:` scheme handler (CSP: same-origin script, workers and fetches only; eval and scripting off), renders a page to a canvas at the zoom and device scale and returns its RGBA, which `PdfPreview` paints. Paging, zoom, fit-to-width, render-while-visible and the page field are ported from `PdfPreview.svelte`; `pdf/logic.rs` is `lib/pdf-preview.ts`.

## View crates

`recur-app` is the window shell: App.svelte's titlebar (breadcrumbs, land chrome, run splits), the native menu, global keys, the overlays host, toasts, boot and shell-state restore. Every main-area view, overlay and palette is a crate under `crates/views/<area>` that plugs into the shell through `recur_view_core::ViewRegistry`. The sidebar is the first one (`recur-view-sidebar`); the shell mounts it directly because it is always on screen.

**Shared state.** `recur_view_core::AppState` is the port of `lib/stores.ts` plus the module state of `lib/api.ts`: projects, groups, workspaces, PRs, inbox, attention, settings, the current route (`view`, `selected_workspace_id`, `selected_pr_id`, `crumb`), overlays, panel sizes, themes, the run store and the command registry. It is one GPUI entity per window, installed by the shell. Read it with `state.read(cx)`, re-render on change with `cx.observe(&state, …)`, and change it through its methods (`go`, `open_workspace`, `open_pr`, `open_project`, `open_new_task`, `set_panel`, `upsert_workspace`, `remove_workspace`, …), which mirror the store functions of the same names. Never cache store data in a view; derive it in `render`.

**Registering a view.** A view crate exports one `register` function that the desktop calls on its registry before the window opens:

```rust
// crates/views/prs/src/lib.rs
use recur_view_core::{OverlaySlot, TitlebarParts, ViewContext, ViewName, ViewRegistry};

pub fn register(r: &mut ViewRegistry) {
    // Actions and key bindings, once at startup (scoped to the view's key context).
    r.init(|cx| {
        cx.bind_keys([gpui_kit::KeyBinding::new("cmd-enter", MergePr, Some("PrDetail"))]);
    });
    // The routes this view renders; the shell builds it once and shows it while one is current.
    r.view(&[ViewName::Pr], |ctx: &ViewContext, window, cx| {
        cx.new(|cx| PrList::new(ctx.clone(), window, cx)).into()
    })
    // Optional titlebar parts: `left` after the breadcrumb, `right` leading `.tb-right`.
    .titlebar(|ctx, _, cx| TitlebarParts {
        left: Some(cx.new(|cx| PrListTitlebar::new(ctx.clone(), cx)).into()),
        right: None,
    });
    // Overlays go in App.svelte's slot so their paint order is preserved.
    r.overlay(OverlaySlot::PrReviewModal, |ctx, window, cx| {
        cx.new(|cx| PrReviewModal::new(ctx.clone(), window, cx)).into()
    });
}

// crates/desktop/src/main.rs
fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_prs::register(&mut r);
    r
}
```

- `view(routes, build)` returns the entry for chaining: `.keep_laid_out()` keeps it laid out (hidden) while another route shows, as `#v-detail` does so editors and terminals keep their size; `.titlebar(f)` adds titlebar parts. Routes nobody registered render an empty main area.
- `overlay(slot, build)`: the overlay view decides its own visibility from `AppState::overlays` (or its own request field, e.g. `schedule_editor`) and renders nothing when closed. The shell paints overlays in `OverlaySlot` order, then toasts, then the title tooltip.
- `init(f)`: actions and key bindings, in the view's own key context. The shell's global keys (`keys.rs`) stand down while focus is in a text-entry context (`Input`, `RecurEditor`, `Terminal`, `TerminalFind`, `TextEntry`); gpui-base inputs, the editor and the terminal already set theirs, and any other element that takes typing sets `key_context("TextEntry")`.

**What a view gets.** `ViewContext { state, services }`: the `AppState` entity and the `Services` handle for backend calls (`services.workspaces.…`) and events (`services.subscribe()`). Prefer AppState's methods over calling services directly when one exists: they apply the same store updates the Svelte API helpers did. Use `recur_view_core::call(cx, fut, "Could not …")` for calls whose failure is an error toast, as `api.ts`' `call()` was.

**Shell events.** `AppState` emits `ShellEvent`s (`cx.subscribe(&state, …)`): `MenuAction(name)` for native menu items and accelerators the view owns (the shell handles the app-level ones first: themes, palettes, navigation, sidebar, settings; views handle `save`, `new-tab`, `close-tab`, panel toggles, focus moves), `PrOpened`, `WorkspaceGone` (drop per-workspace client state), `AppFocused` and `PathOpened`. A view that owns menu items reports their enabled state with `AppState::set_menu_state(MenuState { … })`; the shell rebuilds the native menu from it.

**Hooks back into the shell.** Views that the shell's commands need register themselves on the state: `set_macro_runner` (bundle TypeScript macros), `set_editor_command_handler` and `set_editor_focus_provider` (the focused editor for bundle commands), `set_chat_occluder` (an overlay covers the chat, so it stops counting as seen), and `set_drop_hook` (the desktop allows dropped files).

**Tests and parity.** A view crate tests itself against `MockBridge` the way `crates/app/tests/shell.rs` does: install `AppState` over mock services, open the view (or the whole `Shell` with a registry holding it) in a `VisualTestContext`, find elements by `debug_selector` and drive them with simulated input. Parity scenes for a view go in `rust/tools/ui-parity/scenes.json`; full-window scenes (`shell/…`) build the shell in `recur_app::gallery::build`; add the view to the registry built there to capture it in context.

## Frame cost

GPUI draws a window by rendering its root view, and every view that is not a cached boundary renders again on every frame, whoever notified. `cx.notify()` marks the view and all of its ancestors dirty. A cached view (`Entity::cached(style)` / `AnyView::cached(style)`) is a leaf with a definite size: it is reused while it is not dirty and its bounds, content mask and text style are unchanged, and when it misses it re-renders its whole subtree with caching off, so a cached view nested inside it misses too. Focus changes and input-modality changes (the first key after the mouse) refresh the whole window with caching off. View authors follow these rules:

- **Long lists are virtualized views.** Lay out the rows near the viewport plus overscan and stand in for the rest with spacers at their measured (or estimated) heights, one spacer per run, never one element per off-screen row. Record each laid-out row's height so its stub reserves it. The chat transcript (`recur-view-chat` `window.rs` and `transcript.rs`) and the review's card stack (`recur-view-review` `stack.rs`) are the references.
- **Cache the expensive leaves, not the containers on a notify path.** Anything a frequent notify passes through on its way to the root (the routed view, the Observatory, the workspace view, the chat) renders as a plain child, and its own render stays cheap. What sits beside those paths and is costly is a cached view: the transcript (`TranscriptPane`), the sidebar, the Observatory's editors and previews, the review host. A cached container on the path would refresh every cache inside it on each keystroke.
- **Embed cached views through `style::cached_view`.** GPUI only invalidates a view the window saw accessed in a draw, and a cached view does not count its own render when it was touched earlier in that frame, as on the frame its parent created it. Embedded with plain `.cached(…)`, such a view drops its own notifies (a wheel event, a scroll pin) until something refreshes the whole window; `cached_view` reads it in the parent's render so the window keeps tracking it. Its content root is `style::cached_root` (or carries the tie nudge itself, see below).
- **Split the part that changes often into its own view.** The composer is `DockPane`: composer-only edits call `AgentChat::notify_dock`, which redraws the dock and the views above it but not the cached transcript. Plain `cx.notify()` on the chat redraws both.
- **A render never notifies.** Hosts push props into children from their render (`WorkspaceView::sync`, session cards, the toolbar's pickers), so every setter that can be called from a render compares and returns early when nothing changed (`AgentChat::set_props`, `SessionCard::set_props`, `Select::set_*`, `ModelPickers::set`). A notify during a draw is applied at the next draw, so an unconditional one re-renders the view and its ancestors on every frame, and a cached view around it rebuilds its whole tree each time.
- **Timers and animations run only while they show.** The editor's caret blinks only while it has focus; an unfocused one is drawn steady. A timer that notifies a hidden view invalidates its cached layout.
- **Parked workspace shells are alive but not laid out.** `WorkspaceDetail` keeps the hot shells' entities and runs their effects (`WorkspaceView::sync`) but renders only the bound one: laying a parked shell out again on every switch cost seconds on a large workspace. Anything a shell must show unchanged when it comes back lives in its entities (scroll handles, caches) and is re-applied when it becomes active (`restore_parked_scroll`, the tab strip's `restore_scroll`), since its elements render fresh.
- **Derived data is memoized, not recomputed per render.** Lists, trees and indexes derived from props or stores (the transcript's blocks and estimates, the Observatory's scope and review paths, the review's matched files and file tree, the sidebar's tree and row context) are kept until a generation or the inputs change. Never clone a store collection per render, and never scan a list per item of another (the review stack pruned its card bounds that way: 17M compares a render at 4,210 files).
- **Caches are bounded by bytes.** Size them from measurements and evict least recently used: parsed markdown documents (`recur_content::markdown::DOC_CACHE_MAX_BYTES`, prewarmed off the UI thread), shaped text with its line breaks (`recur_ui::text::SHAPED_MAX_BYTES`), glyph rasters (`recur_ui::glyph`), the transcript UI and chat heap caches. Anything per workspace dies with its shell (only `HOT_SHELL_MAX` stay alive).
- **Hidden subtrees paint beneath the live one.** GPUI still hit-tests an invisible subtree, so a hidden route (`keep_laid_out`) comes first in paint order, as `visibility: hidden` takes no pointer events.
- **Never clone a store record per render.** Read the store by reference (`PrDetail::pr_ref`, slices for a PR's files and commits) and copy out the fields a view keeps; a PR with its patches can be tens of MB, and a clone per accessor per row turns into seconds a frame.
- **Store observers do only their own share.** Every store change wakes every observer: an observer that sorts or scans the whole store (the LSP problem list across all sessions) multiplies by publishes times live views. Memoize per store epoch and look up only what the view shows.
- **Nested flex children multiply layout passes.** Taffy lays a `flex_1` child out once per pass of its parent, and each pass can be at a new width, so cost grows with nesting depth, not node count (a 3-file diff measured its text 67,000 times a frame). A cached view is a fresh layout root at a definite size and resets the count, even when it misses. Its snapped origin rounds sub-pixel edges differently below a fractional offset, so check the parity scenes before adding one there.
- **A cached root keeps WebKit's tie rounding.** A cached view lays out as its own root from a snapped origin, without `style::window_root`'s 1/4096px nudge; add it inside (`style::TIE_NUDGE` spacers, as the transcript column does) or half-pixel edges round the other way. Keep the spacers in columns or content-sized rows: a stretched row around a large subtree lays it out twice.

Measure with `recur_ui::guard::count_renders` / `take_render_counts` (renders per view type, the `frame_cost` specs in the chat and workspace crates assert them) and the real-data smoke's budgets (`RECUR_SMOKE_RENDERS=1` lists what each slow frame rendered).

## Runtime model

- The backend (`engine` and below, `services`) runs on a shared multi-threaded `tokio` runtime: subprocess I/O, sockets, HTTP, timers and file watching. Go goroutines become tasks; `context.Context` cancellation becomes `tokio_util::sync::CancellationToken`; Go channels become `tokio::sync` channels.
- The UI runs on GPUI's foreground executor. `recur-services` exposes async methods (the former Wails-bound methods, same names in snake_case) and a typed `AppEvent` broadcast that replaces the Wails event names. `recur-app` bridges events into GPUI entities with `cx.spawn` and never blocks the foreground thread.
- The Go event bus (`engine/bus.go`) becomes a typed broadcast in `recur-engine`; services translate it to `AppEvent`.

## Services contract

Views talk to the backend only through `recur_services::Services`, a cloneable struct with one `Arc<dyn …>` per former Wails service (`services.workspaces`, `services.prs`, …, field names from Go's `app.Services`). The traits live in `recur_services::api`, one per service, with the Go method names in snake_case and the Go doc comments; `PARITY.md` "Application services" maps every method.

- Every method is `async` and returns `recur_services::Result<T>`. `Error::message` is what the Svelte UI showed from a rejected call. Arguments are borrowed, results owned. Lists and maps are never `null` (Wails' `createFrom` already made them `[]` / `{}`); `GetEditorSession` is the one `Option`.
- DTOs are serde-identical to Go. `recur-domain` and the owning crates' types are used as is (`recur_agent::catalog`, `recur_usage`, `recur_lsp`, the provider status types, `recur_term::Info`) and re-exported from `api`; the `app` package models without another home are defined in `api::models`.
- `services.subscribe()` returns an unbounded receiver of `AppEvent` from that point on, in publish order, never dropped. A view bridges it with `cx.spawn` and matches the variants it cares about. `services.emit(event)` publishes the events the UI itself sent (`menu.state`, `app.ready`, …) for the desktop shell. `AppEvent::name` / `payload` / `from_wire` give the exact Wails event.
- The futures are `Send` and must not assume a tokio context on the caller: GPUI's foreground executor polls them. An implementation that needs tokio spawns onto the runtime handle and awaits the result.
- `recur_services::AppServices` is the real implementation and the composition root (`services/src/app`, Go `internal/app`). `AppServices::open(store)` reads `AppConfig::from_env()` (data dir, worktrees, bundle roots, cache dir, home; nothing else in the crate reads the environment) and registers Claude Code, Codex, Cursor and one compatible provider per Settings endpoint, the GitHub forge and the GitHub Issues and Linear trackers under their persisted ids. `AppServices::new(store, config, AppOptions)` is the test seam (pinned provider, forges, trackers, git setup); `AppConfig::under(root)` puts everything under one temp dir. Every call runs on the tokio handle captured at construction (`rt!` / `rt_ctx!`; `rt_ctx` cancels the call's work when the caller drops it, Go's per-call ctx), so GPUI can poll the futures. The engine bus reaches `AppEvent` through `bridge::attach`, synchronously and in publish order (`app_event` is `AppEvent::from_wire(name, json)`); events only the services raise (`term.*`, `lsp.*`, `workspace.file_index`, `cursor.install`, `provider.usage`, `app.open`) go to the hub directly. Lifecycle for the desktop: `recover()` before any UI, `start()` once events reach the desktop, `set_app_focused`, `shutdown()` on quit. `AssetsService::fetch_asset` replaces the Wails asset middleware (`/workspaces/…`, `/workspace-file`, `/chat-image`, `/external-file`, `/gh-image`) with the same statuses and traversal rules.
- Feature `mock` adds `recur_services::mock`, a port of `frontend/e2e/helpers/mock-bridge.ts`: `MockBridge::fresh(MockOptions::default()).services()` serves the e2e seed with the same mutations, canned replies, chat-attention bookkeeping and post-call events, so a view under the mock reaches the state the Svelte view reaches under e2e. `MockOptions::resolver` overrides one method (a `Reply` value, `BridgeError`, or `Reply::deferred()` to hold a call in flight); `dispatch` / `dispatch_wire` push events the way specs did; `simulate_chat_turn_idle` and `attention()` port the spec helpers. It is for `recur-app` dev builds, tests and the UI-parity harness (`recur-services = { workspace = true, features = ["mock"] }`); release builds leave it off.

```rust
let services = MockBridge::fresh(MockOptions::default()).services(); // or the real Services
let mut events = services.subscribe();
cx.spawn(async move |this, cx| {
    let list = services.workspaces.list_workspaces().await?;
    this.update(cx, |view, cx| view.set_workspaces(list, cx))?;
    while let Some(ev) = events.recv().await {
        if let AppEvent::WorkspaceUpdated(u) = ev {
            this.update(cx, |view, cx| view.patch_workspace(&u, cx))?;
        }
    }
    anyhow::Ok(())
})
.detach();
```

## Conventions

- Edition 2024, toolchain pinned in `rust-toolchain.toml`, workspace lints in `Cargo.toml`. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` stay clean.
- gpui turns on serde_json's `preserve_order` feature for the whole workspace, so `serde_json::Map` keeps insertion order in the app build, while a `-p <crate>` build sorts keys. Go's `json.Marshal` sorts map keys. Never rely on `serde_json::Map` order for output that must match Go: use `BTreeMap` for serialized maps, or sort keys explicitly when re-encoding a `Value`. Run `cargo test --workspace` (feature-unified, as CI does) before finishing, not only `-p`.
- Libraries return typed errors (`thiserror`); `anyhow` only in binaries and at the services boundary.
- `serde` names match the Go JSON tags exactly so `recur.db` JSON columns, `*.json` view-state files and `bundle.toml` round-trip unchanged.
- No `unwrap()` on fallible I/O outside tests. No `todo!()`, `unimplemented!()` or placeholder behavior: a ported item is complete or it is not ticked.
- Port tests with the code: each Go `_test.go` and each frontend `*.test.ts` / e2e spec scenario becomes a Rust test in the owning crate.
- Keep modules small and named after what they do; mirror the Go file split where it reflects real boundaries, merge where it was incidental.
- Shipped content is not copied. `bundles/`, `recur-skills/`, `internal/store/migrations/` and frontend assets (fonts, icons) are embedded from their current repo paths (`include_dir!`, `include_bytes!`). Moving them is part of promotion.

## Side-by-side operation

The two apps must not interfere:

- **Data dir.** Same on-disk formats as the Go app (schema, JSON files, bundle layout), but the Rust app defaults to its own managed root, `<UserConfigDir>/RecurNext`, with worktrees under `~/.recur/worktrees-next`. `RECUR_DATA_DIR` and `RECUR_WORKTREES_DIR` override exactly as in Go, so pointing the Rust app at a copy of real data is one env var. The single-instance lock lives in the data dir, so sharing a dir with the Go app means only one of them runs at a time.
- **Schema guard.** `recur-store` refuses to open a database with migrations it does not know, so the Rust app never writes to a newer schema.
- **Identity.** App name `Recur Next`, its own bundle identifier, and its own open-IPC socket (in its data dir). It does not install the `recur` PATH symlink while running side by side; it installs `recur-next` instead.
- **Build outputs.** Everything builds into `rust/target/`. Nothing writes to `bin/`.

## Promotion

When `PARITY.md` is fully ticked and the Rust app has been the daily driver for a while:

1. Delete `internal/`, `cmd/`, root `*.go`, `go.mod`, `frontend/`, Wails build tasks and Go CI jobs.
2. Move `rust/*` to the repo root and update `include_dir!` paths for `bundles/`, `recur-skills/`, migrations and assets.
3. Switch the default data dir back to `Recur`, worktrees to `~/.recur/worktrees`, identity to `Recur`, and the CLI symlink to `recur`.
