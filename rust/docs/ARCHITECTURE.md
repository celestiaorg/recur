# Recur in Rust

This is the native Rust/GPUI port of Recur. It lives in `rust/` and runs side by side with the Wails app (Go backend in `internal/`, Svelte UI in `frontend/`) until it reaches full parity. Nothing outside `rust/` is changed by the port, except Taskfile and CI entries that build it.

The goal is a faithful port: every feature, behavior, persisted format and keyboard path of the current app carries over, with the same look and feel. Minor native rendering differences are acceptable, and parity never costs performance (`docs/ui-parity.md`). `docs/PARITY.md` is the checklist. An area is done when its items are ticked, its Go and frontend tests are ported, and its screens match within their scenes' tolerances, which guard against regressions.

## Layout

```
rust/
  Cargo.toml            workspace, shared dependency versions, lints
  crates/
    domain/             recur-domain       pure model + rules        (internal/domain, internal/shortid)
    store/              recur-store        SQLite, migrations, paths (internal/store)
    git/                recur-git          local git                 (internal/git)
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
    http/               recur-http         the reqwest client builder (no CA store: HTTPS fails, never panics)
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
      widgets/          recur-view-widgets view pieces several views show (typeaheads, finding rows, pills, Schedules list, …)
      sidebar/          recur-view-sidebar the app sidebar           (Sidebar.svelte, ProviderUsageMeter.svelte)
      <area>/           recur-view-<area>  one crate per route area  (views/*.svelte and the components only they use)
    panes/
      chat/             recur-view-chat    the agent chat            (AgentChat.svelte, AppChat.svelte, cards, pickers)
      review/           recur-view-review  the review surface        (ReviewSurface.svelte, diff cards, threads)
      observatory/      recur-view-observatory the workspace's right side (ObservatoryView.svelte: files, editors, terminals)
    app/                recur-app          the window shell          (App.svelte: titlebar, routing, overlays, menu, keys)
    desktop/            bin `recur`        desktop entry point       (main.go, open_boot.go)
    cli/                bin `recur-cli`    command line              (cmd/recur, internal/cli)
  providers/
    claude/ codex/ cursor/ compatible/ chatgpt/  one crate per agent provider
  tools/
    ui-parity/          recur-ui-parity    parity scenes vs the Svelte captures (docs/ui-parity.md)
    layering/           recur-layering     the layering check below (`lint:rust`)
  docs/
```

Remaining Go packages map as follows: `observe` and `simplify` live as modules of `recur-engine` (or `recur-code` for pure file logic) unless their consumers show a better home; record the final home in `PARITY.md`.

Views live in their own crates under `crates/views/<area>`, and the panes several of them embed under `crates/panes/<pane>` (see "View crates" below). `crates/views` and `crates/panes` have no manifest, so a plain `crates/*` glob would fail: the workspace `members` list `crates/[!pv]*`, `crates/p[!a]*` (`platform`, `project`), `crates/views/*` and `crates/panes/*`.

## Layering

Dependencies point down only (normal dependencies; dev-dependencies are free, since specs mount the whole shell):

```
desktop, cli, tools/ui-parity
app ──────────────► views/sidebar, views/core, ui, services, platform
views/* ──────────► panes/*, views/widgets, views/core, editor, content, textmate, ui, services, engine, forge, code, bundles, agent, domain
panes/* ──────────► views/widgets, views/core, editor, terminal-view, content, textmate, ui, services, engine, lsp, forge, issues, code, bundles, agent, agent-tools, term, domain
views/widgets ────► views/core, content, ui, services, engine, forge, code, domain
views/core ───────► ui, editor, textmate, services, engine, forge, term, domain
content ──────────► editor, textmate, ui
editor ───────────► textmate, bundles, ui
terminal-view ────► term, ui
services ─────────► engine, providers/*, forge, issues, usage, lsp, platform, store, git, code, bundles
engine ───────────► agent, agent-tools, mcp, bundles, code, git, store, term, forge, issues, project, platform
agent, forge, issues, bundles, git, code, store, project ─► domain (and each other, downward)
code ─────────────► textmate (a file's language id)
textmate, term, ui, http ─ standalone
```

Rules:

- `recur-domain` has no I/O and no async.
- Nothing from `recur-services` down knows about GPUI. Only `recur-app`, the view crates, the panes and the widget crates (`recur-ui`, `recur-editor`, `recur-terminal-view`, `recur-content`) render.
- A view crate (`crates/views/<area>`: a route, the palettes, the sidebar) depends on no other view crate except `recur-view-core` and `recur-view-widgets`, and never on `recur-app`. What two views share (state, navigation, pure rules) lives in `recur-view-core`; what they show in common lives in `recur-ui` when it is a generic control, or `recur-view-widgets` when it is a piece of Recur's UI (the typeaheads, finding rows, the Schedules list).
- A pane (`crates/panes/<pane>`) is a large component several routes embed: the chat (workspace, Inbox, home, project settings), the review surface (Observatory, PR detail) and the Observatory (workspace, PR detail's agent dock). Panes register overlays and globals but no routes. A pane depends on `recur-view-core`, `recur-view-widgets` and only the panes it embeds (the Observatory's Review tab is the review surface). When one route needs another's live view, it goes through a hook in `recur-view-core` that the owner installs (`pr_dock::PrDockHost`: PR detail renders the bound workspace's dock).
- Views take pure UI rules from the crate that owns their data (`recur_engine::view`, `recur_code::view`, `recur_forge::view`, the `lib/*.ts` ports), not from each other.
- `recur-engine` depends on the `recur-agent::Provider`, `recur-forge::Forge` and `recur-issues::IssueTracker` traits, never on a concrete implementation. `recur-services` is the composition root that registers the shipped providers, forges and trackers.
- Parity scenes follow the same rules: a whole-window scene takes the shell, the desktop registry and the global reset from the `recur_view_core::gallery::SceneHost` that `tools/ui-parity` installs, so a view's `gallery` feature adds no view or `recur-app` dependency.

`recur-layering`'s tests (run by `lint:rust` and `cargo test --workspace`) fail when a view crate or pane depends on another outside these rules or on `recur-app`, when anything from `recur-services` down depends on GPUI, when the engine reaches a provider crate, or when the engine's non-test sources spell a provider, forge or tracker id (`"claude-code"`, `"codex"`, `"chatgpt"`, `"cursor"`, `"github"`, `"gh"`, `"linear"`, or `"<id>/…"`) or name a concrete forge or tracker module (`recur_forge::github`, `recur_issues::{github,linear}`). Ids the engine needs come from `recur-domain` (`PROVIDER_CLAUDE_CODE`, `PrSource::is_github_alias`).

## Extension points

Adding a provider, forge or issue tracker means adding a crate (providers) or a module (forges, trackers) and one registration line in `recur-services`:

- **Agent providers** implement `recur_agent::Provider` (capabilities, model catalog, flags, start/resume/steer/cancel, normalized event stream, usage). The engine only talks to the trait.
- **Remote git hosting** implements `recur_forge::Forge` (detect from remote URL, list/sync PRs, reviews, threads, reactions, checks, merge, branch rules, mentions, images). GitHub (via `gh`) ships first.
- **Issue trackers** implement `recur_issues::IssueTracker` (search, fetch, expand to task prompt, workspace linking).
- **Bundles** keep their existing on-disk format and semantics; `recur-bundles` is a faithful port.

Registries are keyed by the same string ids the Go app persists, so stored data stays valid: providers `claude-code`, `codex`, `cursor`, `chatgpt` and each compatible provider's own id; forge `github` (PR rows keep `source = "gh"`); trackers `github` and `linear`. See `docs/engine-port-plan.md` section 2. The engine contains no provider, forge or tracker id string compares outside registry lookup.

The engine port is planned slice by slice in `docs/engine-port-plan.md`.

## Editor

`recur-editor` ports Recur's CodeMirror 6 editor (`CodeEditor.svelte`, `MergeEditor.svelte`, `lib/cm-*`) and `recur-textmate` its TextMate stack (`vscode-textmate` semantics on Oniguruma, `tm-*`, `codehl`). `recur-textmate` stands alone: grammars and themes reach it as loaders and parsed JSON, never through `recur-bundles`. The review pane highlights diff rows with the same grammars, off the UI thread (`recur-view-review` `syntax.rs`).

**Architecture.** The editor is not built on gpui-base's text input. It ports CodeMirror's model: an immutable `EditorState` over a rope `Text` (UTF-8 offsets; LSP positions convert to UTF-16), `ChangeSet` / `Transaction` / `Effect`, the selection, history, search, autocomplete, close-brackets, snippet, fold, lint and merge state, and the keymap with its precedence. `EditorView` is a GPUI entity that applies transactions and lays rows out (`display.rs`, wrapping and folds measured through the GPUI text system). One custom `EditorElement` paints in CodeMirror's layer order (scroller fill, selection, line and mark backgrounds, text, cursors, gutters, scrollbars), visible rows only, so large files stay cheap. Chrome around it (panels, tooltips, documentation, the vim panel, gutter icons) is ordinary `recur-ui` elements. vim and emacs are full ports of `@replit/codemirror-vim` 6.3.0 and `@replit/codemirror-emacs` 6.1.0 that run after the editor keymap, as Recur's keymap compartment orders them.

**Highlighting.** `highlight.rs` tokenizes on the UI thread only what the next frame shows: on an edit or first paint, the window's lines from the edit through the line whose end state reconverges with its cached one (`TmEngine::tokenize_viewport`, bounded to the window and `VIEWPORT_BUDGET`), and a far jump past the accurate watermark gets provisional tokens from a guessed start state. Everything past that, the sweep to the end of the document and the re-tokenizing a large edit leaves past the window, runs on GPUI's background executor: the view's fill task takes a `FillJob` (`TmEngine::fill_job`: the grammar, the stack entering the next unfilled line, owned copies of up to `FILL_CHUNK_LINES` lines and their stale stacks), awaits `FillJob::run` there, and hands the `FillChunk` to `TmFill::apply` on the UI thread, which drops it if the document was edited or the watermark moved since the snapshot. That is why `Grammar`, `StateStack` and the compiled scanners are `Send + Sync`: the rule graph is compiled once behind a `OnceLock` (rule ids must match across threads), lazily compiled scanner lists sit behind a `Mutex` held only while compiling, and each thread has its own Oniguruma match region.

**Public API.**

- `EditorView::new(EditorOptions, window, cx)`: `kind` (`Code`, or `Merge { original }` for the Diff view), `path`, `doc`, `saved_doc`, `read_only`, `prefs: EditorPrefs`, `theme: Option<Rc<ParsedTmTheme>>` with `dark_fallback`, `textmate: Option<SharedTextMate>`, `pane`, `snippets: Option<Rc<dyn SnippetProvider>>`, `view_state`, `lit`.
- Host calls: `get_doc` / `peek_doc`, `is_dirty`, `mark_saved`, `apply_saved_baseline`, `replace_doc`, `external_doc_change`, `set_prefs`, `set_theme`, `set_read_only`, `set_lit`, `set_findings`, `set_diff_base`, `set_diagnostics`, `get_view_state` / `apply_view_state`, `scroll_to_line`, `toggle_search`, `apply_snippet_at`, `apply_output`, `mount_path` / `retain_paths` / `forget_path` (multi-buffer views), `coords_at_pos`, `title_at`.
- `EditorEvent`: `DocChanged`, `DirtyChanged`, `Save`, `CopyLine`, `Cursor`, `GotoDefinition`, `SearchOpenChanged`, `OpenFinding`, `PickSnippet`, `Announce`, `Toast`.
- Session and view state: `session` (the `editor-session.ts` shape, caps, wire format and patch gate) and `view_state` (caret, scroll, view mode, dirty rules).
- LSP: the host attaches an `Rc<dyn LspClient>` with `set_lsp(client, uri, language_id)` and pushes diagnostics with `set_lsp_diagnostics`. `LspClient` is the transport-facing half of `@codemirror/lsp-client`: capabilities, `request` (id + response future) and `cancel_request`, `sync`, `open_file` / `close_file` / `file_changed`, `display_file` for cross-file jumps and `apply_workspace_edit`. `lsp::content_changes_for` builds `didChange` bodies.
- Theme inputs: chrome tokens come from the `recur_ui::Theme` global (followed as it changes); code colors from the parsed TextMate theme, which the view makes the catalog's active theme (`TextMate::activate_theme`).

**What hosts provide.** `recur_ui::init` and `recur_ui::Assets`, the view inside `style::window_root` with a `TitleTooltip` as the last child (panel and gutter titles use `recur_ui::tooltip::titled`); a `SharedTextMate` with every grammar registered (`gallery::shipped_textmate` shows the conversion from `recur-bundles`) and the editor theme parsed; a `SnippetProvider` over bundle snippets; handling of `EditorEvent`s (save, toasts, finding and definition navigation); and an `LspClient` adapter over `recur_lsp::Manager` that owns request ids, `initialize`, document versions and `didChange` batching, applying `recur_lsp::protocol` around `Manager::write`. The Observatory's adapter (`lsp/client.rs`) parses server messages and serializes its own on the background executor, in order, so the UI thread handles parsed `Value`s only (a large file's `documentSymbol` or `didChange` is milliseconds of JSON).

## Rich content

`recur-content` is a widget crate, not a view crate: the chat, PR, artifact and file views embed its entities and own their state.

**Markdown.** `marked` 18 (GFM, breaks) and DOMPurify 3 are ported verbatim (`marked/`, `purify/`, JS regex semantics through `regress`, html5ever with scripting off as `DOMParser`), with goldens generated from the JS libraries (`tests/golden/`). `markdown::render_markdown` is `renderMarkdown` (Recur's renderer, emoji, codehl fences, Mermaid placeholders, GitHub image proxying, the `afterSanitizeAttributes` hook); `MarkdownHost` is `markdown-host.ts` (byte-budget cache, worker threads, 2s watchdog). The sanitized HTML is laid out natively (`render/`): the `.chat-canvas` / `.md-doc` stylesheets as data, CSS margin collapsing, WebKit's list markers and table layout, text through `recur_ui::text`, and a mirror of its line layout for link hits and find rects. No HTML from an agent or a PR ever reaches a webview; images load only from `data:` and through the host's `ImageLoader` (the `/gh-image` proxy).

**Embedding.**

- `MarkdownView::new(text, MarkdownClass, demote_images, cx)` (`MarkdownHtml`), `set_text`; events `MarkdownEvent::{LightboxOpen, Toast, TextChanged}`. `StreamingMarkdown::new(cx)`, `set(text, active, complete, cx)`, `set_document_visible`.
- `FilePreview::new(kind, content, path, SharedScroll, window, cx)`: `set_content`, `set_path`, `set_active`, `set_parked`, `toggle_search` / `open_search` / `close_search`; `FilePreviewEvent::SearchOpenChanged`. The view keeps one `SharedScroll` (`PreviewScrollStore`) per workspace, as `createPreviewScrollStore` was.
- `MermaidPreview`, `PdfPreview::new(Option<Arc<[u8]>>, window, cx)` / `set_data`, `HtmlPreview`, `AttachmentLightbox::new(src, name, image, on_close, window, cx)` and `AttachmentLightbox::video(path, name, start, on_close, window, cx)`.
- `video::VideoPlayer::new(path, VideoOptions, cx)`: `play`, `pause`, `seek`, `position`. It decodes through the platform's media framework (AVFoundation on macOS, GStreamer elsewhere), and WebM and Ogg through `video::webm` (libvpx, libaom, theora-rs, libopus, lewton, sound on `cpal`; the Intel slice builds the C codecs from source, `vendor/README.md`), the formats WebKit decodes itself; it paints frames as an image, so it stays in GPUI's paint order. The Linux packages depend on GStreamer's base, good, bad (AV1) and libav plugins.
- Hosts call `set_image_loader` once (GitHub images through the forge), turn `Toast`s into toasts and `LightboxOpen` into `AppState::set_chat_occluder`.
- `sanitize_lsp` is `lib/sanitize-lsp.ts` for HTML-string consumers. recur-editor does not need it: it lays LSP documentation out from pulldown-cmark events and drops raw HTML other than `<br>`.

**Mermaid: mmdr, drawn natively.** `mermaid-rs-renderer` (mmdr, pure Rust) parses the source, lays it out and writes an SVG on the background executor in a few milliseconds, with no webview. Its theme is mermaid's default (light) or dark with the chrome's tokens on top (`mermaid::theme`), in a named font stack (Helvetica Neue first) so mmdr's text measurement and usvg's shaping resolve the same face; the full-canvas background rect is dropped so diagrams sit on their surface, as mermaid.js's did. The SVG is sanitized with the Svelte config, usvg shapes each `<text>`, and every label line becomes `recur_ui::text` at usvg's baseline start, so find highlights and selection work like any other text; labels with rotation or non-color paint stay in the raster. The geometry rasterizes through CoreGraphics (resvg for filters, masks and patterns, and on Linux). mmdr is an independent implementation, so layouts differ from mermaid.js's (edge routing, spacing), which the parity bar of 2026-10-28 allows. mmdr caches the font file it measures with under `~/.cache/mmdr/font-cache` (`XDG_CACHE_HOME`).

**PDF: hayro through `gpui-pdf`.** `pdf/engine.rs` parses the document once with `gpui-pdf` (hayro, pure Rust: no webview, no native library, the same pixels on every platform) and rasterizes a page at the zoom and device scale on the background executor; `PdfPreview` paints the bitmap. hayro is an independent implementation, so pages are not pdf.js's canvas pixel for pixel, which the parity bar of 2026-10-28 allows. Paging, zoom, fit-to-width, render-while-visible and the page field are ported from `PdfPreview.svelte`; `pdf/logic.rs` is `lib/pdf-preview.ts`. Find (⌘F, Rust only) reuses the file preview's bar and matcher over each page's text layer (`gpui-pdf`'s `markup` feature, extracted off the UI thread on first use) and maps each match back to page rects through `PageText::locate`.

## View crates

`recur-app` is the window shell: App.svelte's titlebar (breadcrumbs, land chrome, run splits), the native menu, global keys, the overlays host, toasts, boot and shell-state restore. Every main-area view, overlay and palette is a crate under `crates/views/<area>` (or a pane under `crates/panes/<pane>` for the overlays a pane owns) that plugs into the shell through `recur_view_core::ViewRegistry`. The sidebar is the first one (`recur-view-sidebar`); the shell mounts it directly because it is always on screen.

**Shared state.** `recur_view_core::AppState` is the port of `lib/stores.ts` plus the module state of `lib/api.ts`: projects, groups, workspaces, PRs, inbox, attention, settings, the current route (`view`, `selected_workspace_id`, `selected_pr_id`, `crumb`), overlays, panel sizes, themes, the run store and the command registry. It is one GPUI entity per window, installed by the shell. Read it with `state.read(cx)`, re-render on change with `cx.observe_state(&state, Topics::…, …)` (see "A change reaches only the views that read it" below), and change it through its methods (`go`, `open_workspace`, `open_pr`, `open_project`, `open_new_task`, `set_panel`, `upsert_workspace`, `remove_workspace`, …), which mirror the store functions of the same names. Never cache store data in a view; derive it in `render`.

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

**Tests and parity.** A view crate tests itself against `MockBridge` the way `crates/app/tests/shell.rs` does: install `AppState` over mock services, open the view (or the whole `Shell` with a registry holding it) in a `VisualTestContext`, find elements by `debug_selector` and drive them with simulated input. Parity scenes for a view go in `rust/tools/ui-parity/scenes.json`. Full-window scenes of the shell alone (`shell/…`) are `recur_app::gallery::build`; a view's full-window scenes call `recur_view_core::gallery` (`view_scene`, `build_in`, `scene_seed`), which builds the shell with the desktop registry from the `SceneHost` the parity tool installs, so a new view is captured in context once `tools/ui-parity` registers it.

## Frame cost

GPUI draws a window by rendering its root view, and every view that is not a cached boundary renders again on every frame, whoever notified. `cx.notify()` marks the view and all of its ancestors dirty, except at an isolated view (below). A cached view (`Entity::cached(style)` / `AnyView::cached(style)`) is a leaf with a definite size: it is reused while it is not dirty and its bounds, content mask and text style are unchanged, and when it misses it re-renders its whole subtree with caching off, so a cached view nested inside it misses too. Focus changes and input-modality changes (the first key after the mouse) refresh the whole window with caching off. View authors follow these rules:

- **Long lists are virtualized views.** Lay out the rows near the viewport plus overscan and stand in for the rest with spacers at their measured (or estimated) heights, one spacer per run, never one element per off-screen row. Record each laid-out row's height so its stub reserves it. The chat transcript (`recur-view-chat` `window.rs` and `transcript.rs`) and the review's card stack (`recur-view-review` `stack.rs`) are the references. A long markdown document in a file preview windows itself to its scroller (`MarkdownView::set_scroller`): past 48 top-level blocks it indexes the document once without layout (each block's counters, margins and text blocks, so find still reads every block) and builds only the blocks within a viewport of the visible ones. Rows of one fixed height need no measuring: the source control changes list flattens its rows once per status and lays out the rows near the viewport (`render/scm.rs`).
- **Cache the expensive leaves, not the containers on a notify path.** Anything a frequent notify passes through on its way to the root (the routed view, the Observatory, the workspace view, the chat) renders as a plain child, and its own render stays cheap. What sits beside those paths and is costly is a cached view: the transcript (`TranscriptPane`), the sidebar, the Observatory's previews, the review host (the Observatory's editors are isolated views, below). A cached container on the path would refresh every cache inside it on each keystroke.
- **Embed cached views through `style::cached_view`.** GPUI only invalidates a view the window saw accessed in a draw, and a cached view does not count its own render when it was touched earlier in that frame, as on the frame its parent created it. Embedded with plain `.cached(…)`, such a view drops its own notifies (a wheel event, a scroll pin) until something refreshes the whole window; `cached_view` reads it in the parent's render so the window keeps tracking it. Its content root is `style::cached_root` (or carries the tie nudge itself, see below).
- **Split the part that changes often into its own view.** The composer is `DockPane`, an isolated view: composer-only edits call `AgentChat::notify_dock`, which redraws the dock alone. Plain `cx.notify()` on the chat redraws the chat and the transcript.
- **A render never notifies.** Hosts push props into children from their render (`WorkspaceView::sync`, session cards, the toolbar's pickers), so every setter that can be called from a render compares and returns early when nothing changed (`AgentChat::set_props`, `SessionCard::set_props`, `Select::set_*`, `ModelPickers::set`). A notify during a draw is applied at the next draw, so an unconditional one re-renders the view and its ancestors on every frame, and a cached view around it rebuilds its whole tree each time.
- **Background work on the UI thread runs in slices.** A fill that must run on the foreground (the TextMate outline's tokenizing, `OUTLINE_FILL_SLICE`) takes a few milliseconds per turn, so a key that lands during it waits that long at most. Work that can own its inputs moves off the UI thread instead, as the editor's highlighting fill does (Editor, Highlighting).
- **Timers and animations run only while they show.** The editor's caret blinks only while it has focus; an unfocused one is drawn steady. A timer that notifies a hidden view invalidates its cached layout.
- **Parked workspace shells are alive but not laid out.** `WorkspaceDetail` keeps the hot shells' entities and runs their effects (`WorkspaceView::sync`) but renders only the bound one: laying a parked shell out again on every switch cost seconds on a large workspace. Anything a shell must show unchanged when it comes back lives in its entities (scroll handles, caches) and is re-applied when it becomes active (`restore_parked_scroll`, the tab strip's `restore_scroll`), since its elements render fresh. A fresh element must draw right in its first frame without last frame's layout: a chat `Flow` with nothing placed lays its runs out in prepaint (`layout_root::prepaint_over`), and a `Fit` that has not measured is laid out in place at its content's height.
- **Derived data is memoized, not recomputed per render.** Lists, trees and indexes derived from props or stores (the transcript's blocks and estimates, the Observatory's scope and review paths, the review's matched files and file tree, the sidebar's tree and row context) are kept until a generation or the inputs change. Never clone a store collection per render, and never scan a list per item of another (the review stack pruned its card bounds that way: 17M compares a render at 4,210 files).
- **Caches are bounded by bytes.** Size them from measurements and evict least recently used: parsed markdown documents (`recur_content::markdown::DOC_CACHE_MAX_BYTES`, prewarmed off the UI thread), shaped text with its line breaks (`recur_ui::text::SHAPED_MAX_BYTES`), glyph rasters (`recur_ui::glyph`), the transcript UI and chat heap caches. Anything per workspace dies with its shell (only `HOT_SHELL_MAX` stay alive).
- **Hidden subtrees paint beneath the live one.** GPUI still hit-tests an invisible subtree, so a hidden route (`keep_laid_out`) comes first in paint order, as `visibility: hidden` takes no pointer events.
- **Never clone a store record per render.** Read the store by reference (`PrDetail::pr_ref`, slices for a PR's files and commits) and copy out the fields a view keeps; a PR with its patches can be tens of MB, and a clone per accessor per row turns into seconds a frame.
- **Store observers do only their own share.** Every store change wakes every observer: an observer that sorts or scans the whole store (the LSP problem list across all sessions) multiplies by publishes times live views. Memoize per store epoch and look up only what the view shows.
- **Nested flex children multiply layout passes.** Taffy lays a `flex_1` child out once per pass of its parent, and each pass can be at a new width, so cost grows with nesting depth, not node count (a 3-file diff measured its text 67,000 times a frame). Put a style-sized region behind `recur_ui::layout_root::layout_root`: its content lays out once, as its own root, at the exact sub-pixel edges it reads from layout (no extra frames). A cached view resets the count too, but lays out from a snapped origin: below a fractional offset use `layout_root::cached_block` (next rule) or check the parity scenes.
- **Cache the siblings of a frequent notifier, never its ancestors.** A notify renders the notified view's ancestors, and a cached ancestor that misses re-renders everything under it with caching off. A field is an isolated view (below); for other frequent notifiers (a scroller), make the regions beside it cached panes (`recur_ui::pane::Pane`, `Layer`), keep its ancestors uncached and cheap, and never notify a host from a field's change event: the view that renders a field reads it and redraws on its own.
- **A text field is an isolated view.** Recur vendors gpui-pre with one patch (`rust/vendor/README.md`, `vendor/gpui-pre.patch`): `Entity::isolated(style)` is a cached view whose notifies stop at it. When only isolated views are dirty, the window replays the previous frame (root, cached views and deferred draws) and draws each dirty one again at its recorded place, in its paint order, with its element ids, text style, mask and opacity; a dirty one under a clean cached ancestor redraws inside the ancestor's replay. A key in a field therefore renders the field and nothing else: no host, no ancestor, not the caches inside it (an isolated view keeps them on). Embed one with `style::isolated_view`; a textarea is a `recur_ui::isolated::IsolatedArea` (or a `FieldView` for other fields), the chat's dock a `DockPane` sized by `Fit`, and the Observatory's code and merge editors fill their pane as isolated `EditorView`s, with the status bar's `Ln, Col` a `FieldView` over the caret laid out by an invisible copy of its text (the Observatory renders only when that text's length or the symbol at the caret changes). The contract: an isolated view's size comes from its host, never its content (`textarea_style` is gpui-base's own textarea box, at least rows x line height, so parity holds), and whatever the host derives from it is the host's to redraw. `IsolatedArea` notifies the host only when the value turns empty or not (submit buttons) or the rows change; `Fit` when the measured height changes. A model only isolated views read (a caret's blink state) redraws them alone; one a view outside reads still draws the frame as upstream. The `frame_cost` specs fail when a key in the chat composer, the commit message, a line note, or the PR comment or review box, or the code editor renders any view but the field.
- **Why a gpui-pre patch.** A gpui-base field notifies from its own prepaint (its geometry moves with the caret), and GPUI dirties every ancestor of a notified view, so neither a field in its own cached view, a patched gpui-base input that notified less, nor a Recur input element (which still has to notify a view to repaint) keeps the host from rendering: the host renders on every frame of a key because the window renders from the root. Only the window can replay the frame around a view, so the change is there, opt-in per view, with every other notify drawn as upstream. It replaced the per-field workarounds (a line note's portal still draws its box outside the card stack, now as an isolated view).
- **Fields whose host shows every key stay plain.** A Settings field's page renders on each input event by design (the branch-name example, the section filter, the saved setting); its field stays an ordinary child, and its frame is the page's path alone (`recur-view-settings` `spec::frame_cost`). Filter inputs (palettes, the explorer, quick open) are the same: their host redraws its list per key.
- **The store notifies only on a change.** Every view that reads `AppState` (most panes, the palettes, the review) redraws when it notifies, so a setter that writes back equal data (a sync sweep, a poll) compares first and returns. A notify while typing turns a key into a whole-window redraw.
- **A change reaches only the views that read it.** `AppState`'s frequent setters (agent events, PR sweeps and updates, attention, inbox, briefing, usage, resource, terminal busy/idle, the menu state, panel drags) announce a topic with `AppState::changed(Topics::…)` instead of `cx.notify()`. Views observe the store only through `ObserveState::observe_state(_in)` (never a raw `observe` on it), naming the topics their render reads; an untagged notify still reaches every observer, so a setter that is not tagged stays correct, and a new frequent setter gets a topic. A topic that changes with every agent event in any workspace (workspace rows, attention, PRs) is keyed further in the observer: `observe_state_changes_in` passes the topics, and the view compares what it shows of them (the Observatory's own workspace row, `ObsStoreKey`; the terminal dock's; a diff card's PR facts) before it notifies. A PR row carries a revision (`AppState::pr_rev`) so a view keyed on one PR never compares or clones the row. Whatever a view derives from a large record is the part it shows (`recur_view_core::pr::pr_head`, a card's own notes), kept until that changes. The render-count specs (`spec/frame_cost.rs` in the chat, workspace and PR crates) fail when a key, a store change, a scroll or a tab switch renders a view outside its path.
- **A view that is not on screen is not laid out.** A parked shell, and the bound shell while another route shows, render nothing: the hidden route is one cached view, so any notify inside it (a chat event, a store change) or any whole-window refresh would lay the whole workspace out again under the visible route. Their entities keep running effects through the route (`WorkspaceDetail` observes a hidden bound shell) and re-apply scroll state when they are laid out again (`WorkspaceView::restore_laid_out`).
- **A view released while something still holds it is parked first.** An entity outlives its owner's map when a task holds it (a turn in flight): a shell dropped from the hot list without `set_shell_active(false)` kept its Observatory claiming the menu, and two of them pushed their menu states over each other in an endless store loop.
- **Settle from a draw by notifying the view.** `window.refresh()` during a draw is a no-op in GPUI; an element that learns a size in prepaint and needs another layout calls `cx.notify(window.current_view())` (`exact::settle`, the chat flow).
- **Build HTTP clients through `recur-http`.** `reqwest::Client::new()`, `Client::default()` and a failed `build()` panic or fail where the platform has no CA store; `recur_http::client(|b| …)` falls back to an empty root store, so HTTPS requests fail and plain HTTP works. `recur-layering` fails on a `Client::new()` / `Client::builder()` outside it.
- **Write executables through `recur-exec`.** A file this process writes and then executes is written by a short-lived `/bin/sh` (`recur_exec::write_executable` / `create_executable`): written in-process, a concurrent spawn on another thread inherits the write descriptor and the exec fails with `ETXTBSY`.
- **Spawn subprocesses by path.** std spawns with `posix_spawn` only when the program is a path or the command keeps the parent's `PATH`; a bare name with an explicit `PATH` (`env_clear().envs(..)`) forks, and forking the app holds every malloc lock (the UI thread's too) for as long as copying its page tables takes. Resolve the program first (`recur_git::command::on_path`).
- **Batch many primitives under one layer.** Each painted primitive costs a bounds-tree insert, again whenever a cached view replays it: text paints a line's glyphs in one `paint_layer`. Do the same for any element that paints hundreds of primitives (a minimap, a heatmap).
- **A cached root keeps WebKit's tie rounding.** A cached view lays out as its own root from a snapped origin, without `style::window_root`'s 1/4096px nudge; add it inside (`style::TIE_NUDGE` spacers, as the transcript column does) or half-pixel edges round the other way. Keep the spacers in columns or content-sized rows: a stretched row around a large subtree lays it out twice.

Measure with `recur_ui::guard::count_renders` / `take_render_counts` (renders per view type, the `frame_cost` specs in the chat, workspace, PR and Settings crates assert them) and the real-data smoke's budgets (`RECUR_SMOKE_RENDERS=1` lists what each slow frame rendered).

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
- `recur_services::AppServices` is the real implementation and the composition root (`services/src/app`, Go `internal/app`). `AppServices::open(store)` reads `AppConfig::from_env()` (data dir, worktrees, bundle roots, cache dir, home; nothing else in the crate reads the environment) and registers Claude Code, Codex, Cursor, ChatGPT and one compatible provider per Settings endpoint, the GitHub forge and the GitHub Issues and Linear trackers under their persisted ids. `AppServices::new(store, config, AppOptions)` is the test seam (pinned provider, forges, trackers, git setup); `AppConfig::under(root)` puts everything under one temp dir. Every call runs on the tokio handle captured at construction (`rt!` / `rt_ctx!`; `rt_ctx` cancels the call's work when the caller drops it, Go's per-call ctx), so GPUI can poll the futures. The engine bus reaches `AppEvent` through `bridge::attach`, synchronously and in publish order (`app_event` is `AppEvent::from_wire(name, json)`); events only the services raise (`term.*`, `lsp.*`, `workspace.file_index`, `cursor.install`, `provider.usage`, `app.open`) go to the hub directly. Lifecycle for the desktop: `recover()` before any UI, `start()` once events reach the desktop, `set_app_focused`, `shutdown()` on quit. `AssetsService::fetch_asset` replaces the Wails asset middleware (`/workspaces/…`, `/workspace-file`, `/chat-image`, `/external-file`, `/gh-image`) with the same statuses and traversal rules.
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
- **Packages.** `Recur Next.app` (`build.recur.next`) and the `recur-next` Linux packages (`/opt/recur-next`, `/usr/bin/recur-next`, `build.recur.next.desktop`) install beside the Go app's `Recur.app` and `recur` packages, sharing only the icon and the updater's public key.

## Packaging

Packaging files live in `rust/build/` (not `build/`), so they move with the port on promotion and never touch the Go app's build files. Every output goes to `rust/target/package/`.

- **macOS.** `wails3 task package:rust` runs `rust/build/darwin/package.sh` to build `Recur Next.app`, its DMG and the updater zip. The app has `rust/build/darwin/Info.plist` (the Go plist's keys under the Rust identity, which `identity.rs` tests), the Go app's `icons.icns` / `Assets.car`, and `recur-cli` embedded as `Contents/MacOS/recur-cli`, which cliinstall links as `recur-next`. It builds for `MACOSX_DEPLOYMENT_TARGET` 12.0. Signing follows the Go tasks: ad-hoc or `RECUR_SIGN_IDENTITY` locally, `SIGN_IDENTITY` (+ `ENTITLEMENTS`, `KEYCHAIN_PROFILE` to notarize) through `wails3 tool sign` for releases. `UNIVERSAL=true` builds arm64 + x86_64. The updater turns on when `RECUR_NEXT_UPDATE_URL` names the Rust app's own signed manifest (`RECUR_UPDATE_MANIFEST_URL` at compile time). It is never the Go app's, whose zip is `Recur.app`. `wails3 task smoke:package:rust` installs the DMG into a sandbox, launches it through LaunchServices and checks the `recur-next` link, `recur-next .` and usernoted's delivery of a native notification.
- **Linux.** `rust/build/linux/in-docker.sh` runs a command in `rust/build/linux/Dockerfile` (Debian 13: GPUI's X11/Wayland libraries, GTK 4, WebKitGTK 6.0, Xvfb), or directly on Linux. Its cargo registry and target dir live in `recur-next-*` Docker volumes. `linux:test:rust` runs fmt, clippy and the tests (WebKitGTK ones under `xvfb-run`). `linux:build:rust` writes `recur-next-linux-<arch>/` (`recur`, `recur-cli`, `recur.png`), and `linux:package:rust` adds the tarball plus a `.deb` and `.rpm` from `rust/build/linux/nfpm.yaml`. `linux:smoke:rust` installs both in a clean Debian 13 container and checks paths, the link, the menu entry, `recur-next .` and a notification. `linux:run:rust` serves the app on a noVNC desktop at `http://127.0.0.1:6081` (`rust/build/linux/Dockerfile.desktop`, which reuses the Go desktop's entrypoint and adds Mesa's software Vulkan).
- **CI.** `rust-mac` and `rust-linux` lint and test, and `package-rust-mac` and `package-rust-linux` build the packages (the Linux job also runs `linux:smoke:rust`).
- **Releases.** A `vX.Y.Z` tag builds Recur Next beside the Go app in `.github/workflows/release.yml`, with the same signing, notarization, PGP and updater secrets: `build-next-mac` runs `package:rust` universal (signed, notarized and stapled as the secrets allow) and `build-next-linux` runs `linux:package:rust` on amd64 and arm64 (amd64 also `linux:smoke:rust`). `publish-next` runs after the Go `publish` and only when every Recur Next build passed, and attaches `RecurNext-<v>-macos-universal.{dmg,zip}`, `recur-next-<v>-linux-<arch>.{tar.gz,deb,rpm}`, unversioned latest-download aliases, `SHA256SUMS-next` and `manifest-next.json` (signed by `wails3 updater manifest` over the Recur Next zip only) to the private release and the public `recur-releases` mirror. `RECUR_NEXT_UPDATE_URL` is `releases/latest/download/manifest-next.json` there, so the Rust app never reads the Go app's `manifest.json`, and a Rust failure never holds back the Go release.

### Linux differences

- **HTML preview.** On macOS a WKWebView sits behind GPUI's view, which paints a transparent hole and a native hitbox over it (`vendor/README.md`, "Native views behind the drawing"), so overlays draw over the page and clicks there reach it; GTK 4 cannot embed a widget in a foreign (GPUI) window, so Linux runs an offscreen WebKitGTK page (`html_gtk.rs`), draws its snapshots and forwards input as DOM events. Those events are untrusted, so the shim mirrors `:hover` / `:active` rules onto attributes it sets on the pointer's chain and answers a `<select>` press with its options for a native menu; subframes are refused because WebKitGTK does not say which frame a navigation targets. A WPE WebKit 2.54 page with real input and frame rates sits behind `RECUR_HTML_WPE=1`; its tabled follow-up is `rust/docs/linux-wpe.md`.
- **System UI font.** GPUI's Linux backend maps `.SystemUIFont` to IBM Plex Sans, so `recur_ui::fonts::system_family` asks fontconfig for `system-ui` instead, as WebKitGTK and the Go app do; `Family::System` text uses that face.
- **Color panel** (project icon color): GTK's color chooser dialog, ported as `recur_ui::widgets::color_chooser` in its own window, where macOS opens `NSColorPanel` (`color_panel.rs`).
- **Notifications** go through `notify-send` with no click-through, and there is no dock badge. The Go app does the same.
- **Window icon** comes from the desktop entry, matched by the app id / `WM_CLASS` `build.recur.next` (the Go app also sets it on the window).
- **The updater** runs only in a macOS `.app`, as in the Go app.
- **Backdrop blur.** GPUI's Linux renderer (gpui-pre-wgpu, not vendored) skips backdrop blurs, so the scrims, the maximized terminal dock and the creating-workspace overlay show their color alone (`vendor/README.md`).
- **Keys.** ⌘ chords are Ctrl (GPUI's `secondary`), as `isCmdOrCtrl` makes them in the web UI. As in the Go app, menu items carry no accelerators off macOS, and a focused terminal keeps its Ctrl chords (Ctrl+R, Ctrl+K, Ctrl+T…). The terminal dock's own chords are Ctrl+Shift, and CodeMirror's `Mod` bindings (Ctrl+G find next, Ctrl+End) take precedence over the emacs keymap. Specs press `secondary-…`; the few whose steps only hold on macOS are `ignore`d on Linux, each with the reason beside it.

## Promotion

When `PARITY.md` is fully ticked and the Rust app has been the daily driver for a while:

1. Delete `internal/`, `cmd/`, root `*.go`, `go.mod`, `frontend/`, Wails build tasks and Go CI jobs.
2. Move `rust/*` to the repo root (merging `rust/build/` into `build/`) and update `include_dir!` paths for `bundles/`, `recur-skills/`, migrations and assets.
3. Switch the default data dir back to `Recur`, worktrees to `~/.recur/worktrees`, identity to `Recur`, and the CLI symlink to `recur`.
