# Architecture

Recur is a native Rust app on GPUI (through gpui-kit). It replaced an earlier Go app, and data that app wrote still loads: `recur.db`, the JSON view-state files, `window.json` and `bundle.toml`. Minor native rendering differences are fine, and the look never costs performance (`docs/ui-rendering.md`). Open items are in `docs/FOLLOWUPS.md`.

## Layout

```
Cargo.toml              workspace, shared dependency versions, lints
crates/
  domain/             recur-domain       pure model + rules
  store/              recur-store        SQLite, migrations, paths
  git/                recur-git          local git
  project/            recur-project      project.toml, env, runs
  forge/              recur-forge        remote hosting: GitHub
  issues/             recur-issues       issue trackers: Linear
  agent/              recur-agent        Provider trait, events
  agent-tools/        recur-agent-tools  host tools for agents
  mcp/                recur-mcp          MCP host server
  bundles/            recur-bundles      Recur Bundles
  textmate/           recur-textmate     grammars + themes
  lsp/                recur-lsp          language server client
  term/               recur-term         PTY + emulator state
  http/               recur-http         the reqwest client builder (no CA store: HTTPS fails, never panics)
  code/               recur-code         worktree files and search
  usage/              recur-usage        provider usage polling
  engine/             recur-engine       orchestration
  platform/           recur-platform     OS integration            (power, osfonts, cliinstall, openipc, instance, notifications, updater)
  services/           recur-services     UI-facing API + events
  ui/                 recur-ui           design system on gpui-kit (theme, text, widgets, widget gallery)
  editor/             recur-editor       code + merge editor
  terminal-view/      recur-terminal-view terminal rendering
  content/            recur-content      rich content              (markdown, sanitizing, Mermaid, PDF, file previews, lightbox)
  views/
    core/             recur-view-core    shared app state, routing
    widgets/          recur-view-widgets view pieces several views show (typeaheads, finding rows, pills, Schedules list, …)
    sidebar/          recur-view-sidebar the app sidebar
    <area>/           recur-view-<area>  one crate per route area
  panes/
    chat/             recur-view-chat    the agent chat
    review/           recur-view-review  the review surface
    observatory/      recur-view-observatory the workspace's right side (files, editors, terminals)
  app/                recur-app          the window shell          (titlebar, routing, overlays, menu, keys)
  desktop/            bin `recur`        desktop entry point
  cli/                bin `recur-cli`    command line
providers/
  claude/ codex/ cursor/ compatible/ chatgpt/  one crate per agent provider
tools/
  layering/           recur-layering     the layering check below (`mise run lint`)
vendor/               patched crates.io sources (`vendor/README.md`)
build/                packaging (see "Packaging")
docs/
```

Views live in their own crates under `crates/views/<area>`, and the panes several of them embed under `crates/panes/<pane>` (see "View crates" below). `crates/views` and `crates/panes` have no manifest, so a plain `crates/*` glob would fail: the workspace `members` list `crates/[!pv]*`, `crates/p[!a]*` (`platform`, `project`), `crates/views/*` and `crates/panes/*`.

## Layering

Dependencies point down only (normal dependencies; dev-dependencies are free, since specs mount the whole shell):

```
desktop, cli
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
- Views take pure UI rules from the crate that owns their data (`recur_engine::view`, `recur_code::view`, `recur_forge::view`, `recur_ui::logic`), not from each other.
- `recur-engine` depends on the `recur-agent::Provider`, `recur-forge::Forge` and `recur-issues::IssueTracker` traits, never on a concrete implementation. `recur-services` is the composition root that registers the shipped providers, forges and trackers.

`recur-layering`'s tests (run by `mise run lint` and `cargo test --workspace`) fail when a view crate or pane depends on another outside these rules or on `recur-app`, when anything from `recur-services` down depends on GPUI, when the engine reaches a provider crate, or when the engine's non-test sources spell a provider, forge or tracker id (`"claude-code"`, `"codex"`, `"chatgpt"`, `"cursor"`, `"github"`, `"gh"`, `"linear"`, or `"<id>/…"`) or name a concrete forge or tracker module (`recur_forge::github`, `recur_issues::{github,linear}`). Ids the engine needs come from `recur-domain` (`PROVIDER_CLAUDE_CODE`, `PrSource::is_github_alias`).

**Licenses.** The workspace is Elastic-2.0 (`LICENSE`). A genuinely reusable library with no hosted-product value can be Apache-2.0: set `license = "Apache-2.0"` in its manifest and symlink `LICENSE-APACHE` to the root copy. Today that is `recur-exec`, `recur-http`, `recur-term` and `recur-textmate`, plus `bundles/`, `recur-skills/`, `src-web/`, `src-docs/` and `scripts/` (not crates). A crate that ports third-party code keeps its upstream license notice in a `NOTICE` file. Settings > Licenses lists the third-party software the app includes: `mise run generate:licenses` (a dependency of `dev`, `macos:package` and `linux:build`, skipped while its sources are unchanged) writes `target/licenses.md` from cargo-about's crate list (`build/licenses/about.toml`, which fails on a license it does not accept), `build/licenses/components.md` (native codecs, icons and data cargo does not see; update it when embedding another), the fonts' OFL texts and every `NOTICE`; those tasks point `RECUR_LICENSES_MD` at it and `recur-view-settings`' `build.rs` embeds it. `recur-layering` fails when an Apache-2.0 crate lacks the file or reaches an Elastic-2.0 crate outside dev-dependencies.

## Extension points

Adding a provider, forge or issue tracker means adding a crate (providers) or a module (forges, trackers) and one registration line in `recur-services`:

- **Agent providers** implement `recur_agent::Provider` (capabilities, model catalog, flags, start/resume/steer/cancel, normalized event stream, usage). The engine only talks to the trait.
- **Remote git hosting** implements `recur_forge::Forge` (detect from remote URL, list/sync PRs, reviews, threads, reactions, checks, merge, branch rules, mentions, images). GitHub (via `gh`) ships first.
- **Issue trackers** implement `recur_issues::IssueTracker` (search, fetch, expand to task prompt, workspace linking).
- **Bundles** keep their on-disk format and semantics (`docs/bundles.md`).

Registries are keyed by persisted string ids, so stored data stays valid. The engine contains no provider, forge or tracker id string compares outside registry lookup (`tools/layering` checks this).

| Registry | Persisted ids |
|----------|---------------|
| Providers | `claude-code`, `codex`, `cursor`, `chatgpt`, and each compatible provider's own id. A model ref is `provider/id`, split on the first slash; an unqualified ref is `claude-code`. |
| Forges | Registry key `github`. PR rows keep `source = "gh"`; local PRs are `"local"` and are not a forge. Imported PR rows have the stable id `gh-<projectID>-<number>`. |
| Trackers | `IssueRef.source`: `github`, `linear`. |

## Editor

`recur-editor` is Recur's code and merge editor, a port of CodeMirror 6's behavior, and `recur-textmate` its TextMate stack (`vscode-textmate` semantics on Oniguruma, the per-document engine, `codehl`). `recur-textmate` stands alone: grammars and themes reach it as loaders and parsed JSON, never through `recur-bundles`. The review pane highlights diff rows with the same grammars, off the UI thread (`recur-view-review` `syntax.rs`).

**Architecture.** The editor is not built on gpui-base's text input. It ports CodeMirror's model: an immutable `EditorState` over a rope `Text` (UTF-8 offsets; LSP positions convert to UTF-16), `ChangeSet` / `Transaction` / `Effect`, the selection, history, search, autocomplete, close-brackets, snippet, fold, lint and merge state, and the keymap with its precedence. `EditorView` is a GPUI entity that applies transactions and lays rows out (`display.rs`, wrapping and folds measured through the GPUI text system). One custom `EditorElement` paints in CodeMirror's layer order (scroller fill, selection, line and mark backgrounds, text, cursors, gutters, scrollbars), visible rows only, so large files stay cheap. Chrome around it (panels, tooltips, documentation, the vim panel, gutter icons) is ordinary `recur-ui` elements. vim and emacs are full ports of `@replit/codemirror-vim` 6.3.0 and `@replit/codemirror-emacs` 6.1.0 that run after the editor keymap, as Recur's keymap compartment orders them.

**Highlighting.** `highlight.rs` tokenizes on the UI thread only what the next frame shows: on an edit or first paint, the window's lines from the edit through the line whose end state reconverges with its cached one (`TmEngine::tokenize_viewport`, bounded to the window and `VIEWPORT_BUDGET`), and a far jump past the accurate watermark gets provisional tokens from a guessed start state. Everything past that, the sweep to the end of the document and the re-tokenizing a large edit leaves past the window, runs on GPUI's background executor: the view's fill task takes a `FillJob` (`TmEngine::fill_job`: the grammar, the stack entering the next unfilled line, owned copies of up to `FILL_CHUNK_LINES` lines and their stale stacks), awaits `FillJob::run` there, and hands the `FillChunk` to `TmFill::apply` on the UI thread, which drops it if the document was edited or the watermark moved since the snapshot. That is why `Grammar`, `StateStack` and the compiled scanners are `Send + Sync`: the rule graph is compiled once behind a `OnceLock` (rule ids must match across threads), lazily compiled scanner lists sit behind a `Mutex` held only while compiling, and each thread has its own Oniguruma match region.

**Public API.**

- `EditorView::new(EditorOptions, window, cx)`: `kind` (`Code`, or `Merge { original }` for the Diff view), `path`, `doc`, `saved_doc`, `read_only`, `prefs: EditorPrefs`, `theme: Option<Rc<ParsedTmTheme>>` with `dark_fallback`, `textmate: Option<SharedTextMate>`, `pane`, `snippets: Option<Rc<dyn SnippetProvider>>`, `view_state`, `lit`.
- Host calls: `get_doc` / `peek_doc`, `is_dirty`, `mark_saved`, `apply_saved_baseline`, `replace_doc`, `external_doc_change`, `set_prefs`, `set_theme`, `set_read_only`, `set_lit`, `set_findings`, `set_diff_base`, `set_diagnostics`, `get_view_state` / `apply_view_state`, `scroll_to_line`, `toggle_search`, `apply_snippet_at`, `apply_output`, `mount_path` / `retain_paths` / `forget_path` (multi-buffer views), `coords_at_pos`, `title_at`.
- `EditorEvent`: `DocChanged`, `DirtyChanged`, `Save`, `CopyLine`, `Cursor`, `GotoDefinition`, `SearchOpenChanged`, `OpenFinding`, `PickSnippet`, `Announce`, `Toast`.
- Session and view state: `session` (the editor session's shape, caps, wire format and patch gate) and `view_state` (caret, scroll, view mode, dirty rules).
- LSP: the host attaches an `Rc<dyn LspClient>` with `set_lsp(client, uri, language_id)` and pushes diagnostics with `set_lsp_diagnostics`. `LspClient` is the transport-facing half of `@codemirror/lsp-client`: capabilities, `request` (id + response future) and `cancel_request`, `sync`, `open_file` / `close_file` / `file_changed`, `display_file` for cross-file jumps and `apply_workspace_edit`. `lsp::content_changes_for` builds `didChange` bodies.
- Theme inputs: chrome tokens come from the `recur_ui::Theme` global (followed as it changes); code colors from the parsed TextMate theme, which the view makes the catalog's active theme (`TextMate::activate_theme`).

**What hosts provide.** `recur_ui::init` and `recur_ui::Assets`, the view inside `style::window_root` with a `TitleTooltip` as the last child (panel and gutter titles use `recur_ui::tooltip::titled`); a `SharedTextMate` with every grammar registered (`shipped_textmate` in `crates/editor/examples/editor.rs` shows the conversion from `recur-bundles`) and the editor theme parsed; a `SnippetProvider` over bundle snippets; handling of `EditorEvent`s (save, toasts, finding and definition navigation); and an `LspClient` adapter over `recur_lsp::Manager` that owns request ids, `initialize`, document versions and `didChange` batching, applying `recur_lsp::protocol` around `Manager::write`. The Observatory's adapter (`lsp/client.rs`) parses server messages and serializes its own on the background executor, in order, so the UI thread handles parsed `Value`s only (a large file's `documentSymbol` or `didChange` is milliseconds of JSON).

## Rich content

`recur-content` is a widget crate, not a view crate: the chat, PR, artifact and file views embed its entities and own their state.

**Markdown.** `comrak` (a port of cmark-gfm, so GitHub's own rendering) parses GFM with hard line breaks, and `ammonia` (html5ever) sanitizes its HTML. `markdown::render_markdown` is Recur's renderer: a pass over comrak's tree expands emoji shortcodes outside code, turns fences into codehl `<pre>`s, Mermaid placeholders and `start:end:path` cards, keeps only the text of raw HTML (GitHub-pasted `<img>`s survive), proxies GitHub images through `/gh-image` and unwraps links that are not short http(s) ones. The sanitizer keeps ammonia's allowlist plus task checkboxes, only Recur's own classes and `data:` images; links get `target` and `rel`, and no `id`, `style`, script, handler or frame survives. Mermaid SVG and LSP HTML go through ammonia too; `MarkdownHost` is the settled-markdown cache (byte budget, worker threads, 2s watchdog). The sanitized HTML is laid out natively (`render/`): the `.chat-canvas` / `.md-doc` stylesheets as data, CSS margin collapsing, WebKit's list markers and table layout, text through `recur_ui::text`, and a mirror of its line layout for link hits and find rects. No HTML from an agent or a PR ever reaches a webview; images load only from `data:` and through the host's `ImageLoader` (the `/gh-image` proxy).

**Embedding.**

- `MarkdownView::new(text, MarkdownClass, demote_images, cx)`, `set_text`; events `MarkdownEvent::{LightboxOpen, Toast, TextChanged}`. `StreamingMarkdown::new(cx)`, `set(text, active, complete, cx)`, `set_document_visible`.
- `FilePreview::new(kind, content, path, SharedScroll, window, cx)`: `set_content`, `set_path`, `set_active`, `set_parked`, `toggle_search` / `open_search` / `close_search`; `FilePreviewEvent::SearchOpenChanged`. The view keeps one `SharedScroll` (`PreviewScrollStore`) per workspace.
- `MermaidPreview`, `PdfPreview::new(Option<Arc<[u8]>>, window, cx)` / `set_data`, `HtmlPreview`, `AttachmentLightbox::new(src, name, image, on_close, window, cx)` and `AttachmentLightbox::video(path, name, start, on_close, window, cx)`.
- `video::VideoPlayer::new(path, VideoOptions, cx)`: `play`, `pause`, `seek`, `position`. It decodes through the platform's media framework (AVFoundation on macOS, GStreamer elsewhere), and WebM and Ogg through `video::webm` (libvpx, libaom, theora-rs, libopus, lewton, sound on `cpal`; the Intel slice builds the C codecs from source, `vendor/README.md`), the formats WebKit decodes itself; it paints frames as an image, so it stays in GPUI's paint order. The Linux packages depend on GStreamer's base, good, bad (AV1) and libav plugins.
- Hosts call `set_image_loader` once (GitHub images through the forge), turn `Toast`s into toasts and `LightboxOpen` into `AppState::set_chat_occluder`.
- `sanitize_lsp` sanitizes language-server HTML for HTML-string consumers. recur-editor does not need it: it lays LSP documentation out from pulldown-cmark events and drops raw HTML other than `<br>`.

**Mermaid: mmdr, drawn natively.** `mermaid-rs-renderer` (mmdr, pure Rust) parses the source, lays it out and writes an SVG on the background executor in a few milliseconds, with no webview. Its theme is mermaid's default (light) or dark with the chrome's tokens on top (`mermaid::theme`), in a named font stack (Helvetica Neue first) so mmdr's text measurement and usvg's shaping resolve the same face; the full-canvas background rect is dropped so diagrams sit on their surface. The SVG is sanitized with the Mermaid-SVG config, usvg shapes each `<text>`, and every label line becomes `recur_ui::text` at usvg's baseline start, so find highlights and selection work like any other text; labels with rotation or non-color paint stay in the raster. The geometry rasterizes through resvg. mmdr is an independent implementation, so layouts differ from mermaid.js's (edge routing, spacing). mmdr caches the font file it measures with under `~/.cache/mmdr/font-cache` (`XDG_CACHE_HOME`).

**PDF: hayro through `gpui-pdf`.** `pdf/engine.rs` parses the document once with `gpui-pdf` (hayro, pure Rust: no webview, no native library, the same pixels on every platform) and rasterizes a page at the zoom and device scale on the background executor; `PdfPreview` paints the bitmap. hayro is an independent implementation, so pages are not pdf.js's canvas pixel for pixel. `pdf/view.rs` handles paging, zoom, fit-to-width, render-while-visible and the page field, with the arithmetic in `pdf/logic.rs`. Find (⌘F) reuses the file preview's bar and matcher over each page's text layer (`gpui-pdf`'s `markup` feature, extracted off the UI thread on first use) and maps each match back to page rects through `PageText::locate`.

## View crates

`recur-app` is the window shell: the titlebar (breadcrumbs, land chrome, run splits), the native menu, global keys, the overlays host, toasts, boot and shell-state restore. Every main-area view, overlay and palette is a crate under `crates/views/<area>` (or a pane under `crates/panes/<pane>` for the overlays a pane owns) that plugs into the shell through `recur_view_core::ViewRegistry`. The sidebar is the first one (`recur-view-sidebar`); the shell mounts it directly because it is always on screen.

**Shared state.** `recur_view_core::AppState` holds the app-wide stores and data layer: projects, groups, workspaces, PRs, inbox, attention, settings, the current route (`view`, `selected_workspace_id`, `selected_pr_id`, `crumb`), overlays, panel sizes, themes, the run store and the command registry. It is one GPUI entity per window, installed by the shell. Read it with `state.read(cx)`, re-render on change with `cx.observe_state(&state, Topics::…, …)` (see "A change reaches only the views that read it" below), and change it through its methods (`go`, `open_workspace`, `open_pr`, `open_project`, `open_new_task`, `set_panel`, `upsert_workspace`, `remove_workspace`, …). Never cache store data in a view; derive it in `render`.

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
    // Overlays go in a fixed slot so their paint order is preserved.
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

- `view(routes, build)` returns the entry for chaining: `.keep_laid_out()` keeps it laid out (hidden) while another route shows, as the workspace detail route does so editors and terminals keep their size; `.titlebar(f)` adds titlebar parts. Routes nobody registered render an empty main area.
- `overlay(slot, build)`: the overlay view decides its own visibility from `AppState::overlays` (or its own request field, e.g. `schedule_editor`) and renders nothing when closed. The shell paints overlays in `OverlaySlot` order, then toasts, then the title tooltip.
- `init(f)`: actions and key bindings, in the view's own key context. The shell's global keys (`keys.rs`) stand down while focus is in a text-entry context (`Input`, `RecurEditor`, `Terminal`, `TerminalFind`, `TextEntry`); gpui-base inputs, the editor and the terminal already set theirs, and any other element that takes typing sets `key_context("TextEntry")`.

**What a view gets.** `ViewContext { state, services }`: the `AppState` entity and the `Services` handle for backend calls (`services.workspaces.…`) and events (`services.subscribe()`). Prefer AppState's methods over calling services directly when one exists: they also apply the matching store updates. Use `recur_view_core::call(cx, fut, "Could not …")` for calls whose failure is an error toast, as `api.ts`' `call()` was.

**Shell events.** `AppState` emits `ShellEvent`s (`cx.subscribe(&state, …)`): `MenuAction(name)` for native menu items and accelerators the view owns (the shell handles the app-level ones first: themes, palettes, navigation, sidebar, settings; views handle `save`, `new-tab`, `close-tab`, panel toggles, focus moves), `PrOpened`, `WorkspaceGone` (drop per-workspace client state), `AppFocused` and `PathOpened`. A view that owns menu items reports their enabled state with `AppState::set_menu_state(MenuState { … })`; the shell rebuilds the native menu from it.

**Hooks back into the shell.** Views that the shell's commands need register themselves on the state: `set_macro_runner` (bundle TypeScript macros), `set_editor_command_handler` and `set_editor_focus_provider` (the focused editor for bundle commands), `set_chat_occluder` (an overlay covers the chat, so it stops counting as seen), and `set_drop_hook` (the desktop allows dropped files).

**Tests.** A view crate tests itself against `recur_services::fake::Fake` the way `crates/app/tests/shell.rs` does: install `AppState` over the fake's services, open the view (or the whole `Shell` with a registry holding it) in a `VisualTestContext`, find elements by `debug_selector` and drive them with simulated input.

## Frame cost

GPUI draws a window by rendering its root view, and every view that is not a cached boundary renders again on every frame, whoever notified. `cx.notify()` marks the view and all of its ancestors dirty, except at an isolated view (below). A cached view (`Entity::cached(style)` / `AnyView::cached(style)`) is a leaf with a definite size: it is reused while it is not dirty and its bounds, content mask and text style are unchanged, and when it misses it re-renders its whole subtree with caching off, so a cached view nested inside it misses too. Focus changes and input-modality changes (the first key after the mouse) refresh the whole window with caching off. View authors follow these rules:

- **Long lists are virtualized views.** Lay out the rows near the viewport plus overscan and stand in for the rest with spacers at their measured (or estimated) heights, one spacer per run, never one element per off-screen row. Record each laid-out row's height so its stub reserves it. The chat transcript (`recur-view-chat` `window.rs` and `transcript.rs`) and the review's card stack (`recur-view-review` `stack.rs`) are the references. A long markdown document in a file preview windows itself to its scroller (`MarkdownView::set_scroller`): past 48 top-level blocks it indexes the document once without layout (each block's counters, margins and text blocks, so find still reads every block) and builds only the blocks within a viewport of the visible ones. Rows of one fixed height need no measuring: the source control changes list flattens its rows once per status and lays out the rows near the viewport (`render/scm.rs`).
- **Cache the expensive leaves, not the containers on a notify path.** Anything a frequent notify passes through on its way to the root (the routed view, the Observatory, the workspace view, the chat) renders as a plain child, and its own render stays cheap. What sits beside those paths and is costly is a cached view: the transcript (`TranscriptPane`), the sidebar, the Observatory's previews, the review host (the Observatory's editors are isolated views, below). A cached container on the path would refresh every cache inside it on each keystroke.
- **Embed cached views through `style::cached_view`.** GPUI only invalidates a view the window saw accessed in a draw, and a cached view does not count its own render when it was touched earlier in that frame, as on the frame its parent created it. Embedded with plain `.cached(…)`, such a view drops its own notifies (a wheel event, a scroll pin) until something refreshes the whole window; `cached_view` reads it in the parent's render so the window keeps tracking it. Its content root is `style::cached_root`.
- **Split the part that changes often into its own view.** The composer is `DockPane`, an isolated view: composer-only edits call `AgentChat::notify_dock`, which redraws the dock alone. Plain `cx.notify()` on the chat redraws the chat and the transcript.
- **A render never notifies.** Hosts push props into children from their render (`WorkspaceView::sync`, session cards, the toolbar's pickers), so every setter that can be called from a render compares and returns early when nothing changed (`AgentChat::set_props`, `SessionCard::set_props`, `Select::set_*`, `ModelPickers::set`). A notify during a draw is applied at the next draw, so an unconditional one re-renders the view and its ancestors on every frame, and a cached view around it rebuilds its whole tree each time.
- **Background work on the UI thread runs in slices.** A fill that must run on the foreground (the TextMate outline's tokenizing, `OUTLINE_FILL_SLICE`) takes a few milliseconds per turn, so a key that lands during it waits that long at most. Work that can own its inputs moves off the UI thread instead, as the editor's highlighting fill does (Editor, Highlighting).
- **Timers and animations run only while they show.** The editor's caret blinks only while it has focus; an unfocused one is drawn steady. A timer that notifies a hidden view invalidates its cached layout.
- **Parked workspace shells are alive but not laid out.** `WorkspaceDetail` keeps the hot shells' entities and runs their effects (`WorkspaceView::sync`) but renders only the bound one: laying a parked shell out again on every switch cost seconds on a large workspace. Anything a shell must show unchanged when it comes back lives in its entities (scroll handles, caches) and is re-applied when it becomes active (`restore_parked_scroll`, the tab strip's `restore_scroll`), since its elements render fresh. A fresh element must draw right in its first frame without last frame's layout: a chat `Flow` with nothing placed lays its runs out in prepaint (`layout_root::prepaint_over`), and a `Fit` that has not measured is laid out in place at its content's height.
- **Derived data is memoized, not recomputed per render.** Lists, trees and indexes derived from props or stores (the transcript's blocks and estimates, the Observatory's scope and review paths, the review's matched files and file tree, the sidebar's tree and row context) are kept until a generation or the inputs change. Never clone a store collection per render, and never scan a list per item of another (the review stack pruned its card bounds that way: 17M compares a render at 4,210 files).
- **Caches are bounded by bytes.** Size them from measurements and evict least recently used: parsed markdown documents (`recur_content::markdown::DOC_CACHE_MAX_BYTES`, prewarmed off the UI thread), shaped text with its line breaks (`recur_ui::text::SHAPED_MAX_BYTES`), the transcript UI and chat heap caches. Anything per workspace dies with its shell (only `HOT_SHELL_MAX` stay alive).
- **Hidden subtrees paint beneath the live one.** GPUI still hit-tests an invisible subtree, so a hidden route (`keep_laid_out`) comes first in paint order, as `visibility: hidden` takes no pointer events.
- **Never clone a store record per render.** Read the store by reference (`PrDetail::pr_ref`, slices for a PR's files and commits) and copy out the fields a view keeps; a PR with its patches can be tens of MB, and a clone per accessor per row turns into seconds a frame.
- **Store observers do only their own share.** Every store change wakes every observer: an observer that sorts or scans the whole store (the LSP problem list across all sessions) multiplies by publishes times live views. Memoize per store epoch and look up only what the view shows.
- **Nested flex children multiply layout passes.** Taffy lays a `flex_1` child out once per pass of its parent, and each pass can be at a new width, so cost grows with nesting depth, not node count (a 3-file diff measured its text 67,000 times a frame). Put a style-sized region behind `recur_ui::layout_root::layout_root`: its content lays out once, as its own root, at the exact sub-pixel edges it reads from layout (no extra frames). A cached view resets the count too, but lays out from a snapped origin: below a fractional offset use `layout_root::cached_block` (next rule).
- **Cache the siblings of a frequent notifier, never its ancestors.** A notify renders the notified view's ancestors, and a cached ancestor that misses re-renders everything under it with caching off. A field is an isolated view (below); for other frequent notifiers (a scroller), make the regions beside it cached panes (`recur_ui::pane::Pane`, `Layer`), keep its ancestors uncached and cheap, and never notify a host from a field's change event: the view that renders a field reads it and redraws on its own.
- **A text field is an isolated view.** Recur vendors gpui-pre with one patch (`vendor/README.md`, `vendor/gpui-pre.patch`): `Entity::isolated(style)` is a cached view whose notifies stop at it. When only isolated views are dirty, the window replays the previous frame (root, cached views and deferred draws) and draws each dirty one again at its recorded place, in its paint order, with its element ids, text style, mask and opacity; a dirty one under a clean cached ancestor redraws inside the ancestor's replay. A key in a field therefore renders the field and nothing else: no host, no ancestor, not the caches inside it (an isolated view keeps them on). Embed one with `style::isolated_view`; a textarea is a `recur_ui::isolated::IsolatedArea` (or a `FieldView` for other fields), the chat's dock a `DockPane` sized by `Fit`, and the Observatory's code and merge editors fill their pane as isolated `EditorView`s, with the status bar's `Ln, Col` a `FieldView` over the caret laid out by an invisible copy of its text (the Observatory renders only when that text's length or the symbol at the caret changes). The contract: an isolated view's size comes from its host, never its content (`textarea_style` is gpui-base's own textarea box, at least rows x line height, so the size matches), and whatever the host derives from it is the host's to redraw. `IsolatedArea` notifies the host only when the value turns empty or not (submit buttons) or the rows change; `Fit` when the measured height changes. A model only isolated views read (a caret's blink state) redraws them alone; one a view outside reads still draws the frame as upstream. The `frame_cost` specs fail when a key in the chat composer, the commit message, a line note, or the PR comment or review box, or the code editor renders any view but the field.
- **Why a gpui-pre patch.** A gpui-base field notifies from its own prepaint (its geometry moves with the caret), and GPUI dirties every ancestor of a notified view, so neither a field in its own cached view, a patched gpui-base input that notified less, nor a Recur input element (which still has to notify a view to repaint) keeps the host from rendering: the host renders on every frame of a key because the window renders from the root. Only the window can replay the frame around a view, so the change is there, opt-in per view, with every other notify drawn as upstream. It replaced the per-field workarounds (a line note's portal still draws its box outside the card stack, now as an isolated view).
- **Fields whose host shows every key stay plain.** A Settings field's page renders on each input event by design (the branch-name example, the section filter, the saved setting); its field stays an ordinary child, and its frame is the page's path alone (`recur-view-settings` `spec::frame_cost`). Filter inputs (palettes, the explorer, quick open) are the same: their host redraws its list per key.
- **The store notifies only on a change.** Every view that reads `AppState` (most panes, the palettes, the review) redraws when it notifies, so a setter that writes back equal data (a sync sweep, a poll) compares first and returns. A notify while typing turns a key into a whole-window redraw.
- **A change reaches only the views that read it.** `AppState`'s frequent setters (agent events, PR sweeps and updates, attention, inbox, briefing, usage, resource, terminal busy/idle, the menu state, panel drags) announce a topic with `AppState::changed(Topics::…)` instead of `cx.notify()`. Views observe the store only through `ObserveState::observe_state(_in)` (never a raw `observe` on it), naming the topics their render reads; an untagged notify still reaches every observer, so a setter that is not tagged stays correct, and a new frequent setter gets a topic. A topic that changes with every agent event in any workspace (workspace rows, attention, PRs) is keyed further in the observer: `observe_state_changes_in` passes the topics, and the view compares what it shows of them (the Observatory's own workspace row, `ObsStoreKey`; the terminal dock's; a diff card's PR facts) before it notifies. A PR row carries a revision (`AppState::pr_rev`) so a view keyed on one PR never compares or clones the row. Whatever a view derives from a large record is the part it shows (`recur_view_core::pr::pr_head`, a card's own notes), kept until that changes. The render-count specs (`spec/frame_cost.rs` in the chat, workspace and PR crates) fail when a key, a store change, a scroll or a tab switch renders a view outside its path.
- **A view that is not on screen is not laid out.** A parked shell, and the bound shell while another route shows, render nothing: the hidden route is one cached view, so any notify inside it (a chat event, a store change) or any whole-window refresh would lay the whole workspace out again under the visible route. Their entities keep running effects through the route (`WorkspaceDetail` observes a hidden bound shell) and re-apply scroll state when they are laid out again (`WorkspaceView::restore_laid_out`).
- **A view released while something still holds it is parked first.** An entity outlives its owner's map when a task holds it (a turn in flight): a shell dropped from the hot list without `set_shell_active(false)` kept its Observatory claiming the menu, and two of them pushed their menu states over each other in an endless store loop.
- **Settle from a draw by notifying the view.** `window.refresh()` during a draw is a no-op in GPUI; an element that learns a size in prepaint and needs another layout calls `cx.notify(window.current_view())` (the chat flow).
- **Build HTTP clients through `recur-http`.** `reqwest::Client::new()`, `Client::default()` and a failed `build()` panic or fail where the platform has no CA store; `recur_http::client(|b| …)` falls back to an empty root store, so HTTPS requests fail and plain HTTP works. `recur-layering` fails on a `Client::new()` / `Client::builder()` outside it.
- **Write executables through `recur-exec`.** A file this process writes and then executes is written by a short-lived `/bin/sh` (`recur_exec::write_executable` / `create_executable`): written in-process, a concurrent spawn on another thread inherits the write descriptor and the exec fails with `ETXTBSY`.
- **Spawn subprocesses by path.** std spawns with `posix_spawn` only when the program is a path or the command keeps the parent's `PATH`; a bare name with an explicit `PATH` (`env_clear().envs(..)`) forks, and forking the app holds every malloc lock (the UI thread's too) for as long as copying its page tables takes. Resolve the program first (`recur_git::command::on_path`).
- **Batch many primitives under one layer.** Each painted primitive costs a bounds-tree insert, again whenever a cached view replays it: text paints a line's glyphs in one `paint_layer`. Do the same for any element that paints hundreds of primitives (a minimap, a heatmap).

Measure with `recur_ui::guard::count_renders` / `take_render_counts` (renders per view type, the `frame_cost` specs in the chat, workspace, PR and Settings crates assert them) and the real-data smoke's budgets (`RECUR_SMOKE_RENDERS=1` lists what each slow frame rendered).

## Runtime model

- The backend (`engine` and below, `services`) runs on a shared multi-threaded `tokio` runtime: subprocess I/O, sockets, HTTP, timers and file watching. Cancellation is `tokio_util::sync::CancellationToken`; channels are `tokio::sync` channels.
- The UI runs on GPUI's foreground executor. `recur-services` exposes async methods and a typed `AppEvent` broadcast. `recur-app` bridges events into GPUI entities with `cx.spawn` and never blocks the foreground thread.
- The engine bus is a typed broadcast in `recur-engine`; services translate it to `AppEvent`.

## Services contract

Views talk to the backend only through `recur_services::Services`, a cloneable struct with one `Arc<dyn …>` per service (`services.workspaces`, `services.prs`, …). The traits live in `recur_services::api`, one per service.

- Every method is `async` and returns `recur_services::Result<T>`. `Error::message` is what views show for a failed call. Arguments are borrowed, results owned. Lists and maps are never `null`; `GetEditorSession` is the one `Option`.
- DTOs keep the Go app's JSON field names, so state it persisted still decodes. `recur-domain` and the owning crates' types are used as is (`recur_agent::catalog`, `recur_usage`, `recur_lsp`, the provider status types, `recur_term::Info`) and re-exported from `api`; the `app` package models without another home are defined in `api::models`.
- `services.subscribe()` returns an unbounded receiver of `AppEvent` from that point on, in publish order, never dropped. A view bridges it with `cx.spawn` and matches the variants it cares about. `services.emit(event)` publishes the events the UI itself sent (`menu.state`, `app.ready`, …) for the desktop shell. `AppEvent::name` / `payload` / `from_wire` give an event's wire name and JSON data.
- The futures are `Send` and must not assume a tokio context on the caller: GPUI's foreground executor polls them. An implementation that needs tokio spawns onto the runtime handle and awaits the result.
- `recur_services::AppServices` is the real implementation and the composition root (`services/src/app`). `AppServices::open(store)` reads `AppConfig::from_env()` (data dir, worktrees, bundle roots, cache dir, home; nothing else in the crate reads the environment) and registers Claude Code, Codex, Cursor, ChatGPT and one compatible provider per Settings endpoint, the GitHub forge and the GitHub Issues and Linear trackers under their persisted ids. `AppServices::new(store, config, AppOptions)` is the test seam (pinned provider, forges, trackers, git setup); `AppConfig::under(root)` puts everything under one temp dir. Every call runs on the tokio handle captured at construction (`rt!` / `rt_ctx!`; `rt_ctx` cancels the call's work when the caller drops it), so GPUI can poll the futures. The engine bus reaches `AppEvent` through `bridge::attach`, synchronously and in publish order (`app_event` is `AppEvent::from_wire(name, json)`); events only the services raise (`term.*`, `lsp.*`, `workspace.file_index`, `cursor.install`, `provider.usage`, `app.open`) go to the hub directly. Lifecycle for the desktop: `recover()` before any UI, `start()` once events reach the desktop, `set_app_focused`, `shutdown()` on quit. `AssetsService::fetch_asset` serves the asset routes (`/workspaces/…`, `/workspace-file`, `/chat-image`, `/external-file`, `/gh-image`) with the same statuses and traversal rules.
- Feature `fake` adds `recur_services::fake`: `Fake` implements every service trait over a typed `Seed` (the e2e store dump in `fake/seed.json` plus what calls add: chat tabs, editor sessions, shell state). `Fake::fresh(Overrides::default()).services()` answers with the seed's rows, in-memory mutations, canned answers or empty values, plus chat-attention bookkeeping and the events a call queues. `Overrides` has one builder per method (`Overrides::default().read_file(|seed, (project, ws, path)| Ok(…))`): the closure gets the seed and the call's owned arguments and returns a `Result`, or `fake::hold()`'s answer to keep the call in flight until its `Release` sends one. `dispatch` pushes an `AppEvent`; `simulate_chat_turn_idle`, `attention()` and `with_seed` are spec helpers. One macro (`fake_services!` in `fake/mod.rs`) generates the trait impls, the default bodies and the override slots. It is for `recur-desktop --features fake` and the view specs (`recur-services = { workspace = true, features = ["fake"] }`); release builds leave it off.

```rust
let services = Fake::fresh(Overrides::default()).services(); // or the real Services
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
- gpui turns on serde_json's `preserve_order` feature for the whole workspace, so `serde_json::Map` keeps insertion order in the app build, while a `-p <crate>` build sorts keys. Never rely on `serde_json::Map` order for output that must be deterministic: use `BTreeMap` for serialized maps, or sort keys explicitly when re-encoding a `Value`. Run `cargo test --workspace` (feature-unified, as CI does) before finishing, not only `-p`.
- Libraries return typed errors (`thiserror`); `anyhow` only in binaries and at the services boundary.
- `serde` names keep the existing JSON and TOML field names, so `recur.db` JSON columns, `*.json` view-state files, `window.json` and `bundle.toml` written by the Go app still read.
- No `unwrap()` on fallible I/O outside tests. No `todo!()`, `unimplemented!()` or placeholder behavior.
- Keep modules small and named after what they do.
- Shipped content is not copied. `bundles/` and `recur-skills/` are embedded from their repo paths (`include_dir!`); migrations, file icons, the emoji table and other assets live in the crate that embeds them.

## Identity and data

- **Data dir.** The app replaced the Go app and keeps its managed root, `<UserConfigDir>/Recur`, with worktrees under `~/.recur/worktrees`, so existing installs open their data as-is. `RECUR_DATA_DIR` and `RECUR_WORKTREES_DIR` override them, so pointing the app at a copy of real data is one env var. The single-instance lock and the open-IPC socket live in the data dir.
- **Schema guard.** `recur-store` refuses to open a database with migrations it does not know, so the app never writes to a newer schema.
- **Identity.** Shared with the Go app: app name `Recur`, bundle identifier and Linux app id `build.recur.app`, the `recur` PATH symlink to `recur-cli`, and the `recur` Linux package in `/opt/recur`. Releases publish under the Go app's asset names and `manifest.json`; the Go app's macOS updater reads the same manifest, so it updates into this app.
- **Build outputs.** Everything builds into `target/`; packages go to `target/package/`.

## Packaging

Packaging files live in `build/`, tools in `mise.toml` and tasks in `tasks/*.toml`.

- **macOS.** `mise run macos:package` compiles `build/appicon.icon` with `actool` (`generate:icons`: `build/darwin/Assets.car` and `icons.icns`), copies the user docs into the `recur-help` skill (`generate:help-skill`), then runs `build/darwin/package.sh` to build `Recur.app`, its DMG and the updater zip. The app has `build/darwin/Info.plist` (which `identity.rs` tests), `icons.icns` / `Assets.car`, and `recur-cli` embedded as `Contents/MacOS/recur-cli`, which cliinstall links as `recur`. It builds for `MACOSX_DEPLOYMENT_TARGET` 12.0. Signing is ad-hoc or `RECUR_SIGN_IDENTITY` locally; for releases `SIGN_IDENTITY` (+ `ENTITLEMENTS`) signs the CLI and then the bundle with `codesign --timestamp --options runtime`, and `KEYCHAIN_PROFILE` (an `xcrun notarytool store-credentials` profile) notarizes and staples the app and the DMG. `UNIVERSAL=true` builds arm64 + x86_64. The DMG is laid out by dmgbuild (`build/darwin/dmg-settings.py`: a 540x380 window, 96 px icons, the app and an Applications link side by side over `dmg-background.tiff`). The updater turns on when `RECUR_UPDATE_URL` names the signed manifest (`RECUR_UPDATE_MANIFEST_URL` at compile time). `mise run macos:smoke` installs the DMG into a sandbox, launches it through LaunchServices and checks the `recur` link, `recur .` and usernoted's delivery of a native notification.
- **Linux.** `build/linux/in-docker.sh` runs a command in `build/linux/Dockerfile` (Ubuntu 24.04 like CI's runner: GPUI's X11/Wayland libraries, GTK 4, WebKitGTK 6.0, Xvfb), or directly on Linux. Its cargo registry and target dir live in `recur-linux-*` Docker volumes. `linux:test` runs fmt, clippy and the tests (WebKitGTK ones under `xvfb-run`). `linux:build` writes `recur-linux-<arch>/` (`recur`, `recur-cli`, `recur.png`), and `linux:package` adds the tarball plus a `.deb` and `.rpm` (`nfpm pkg` with `build/linux/nfpm.yaml`). `linux:smoke` installs both in a clean Debian 13 container and checks paths, the link, the menu entry, `recur .` and a notification. `linux:run` serves the app on a noVNC desktop at `http://127.0.0.1:6081` (`build/linux/Dockerfile.desktop` with Mesa's software Vulkan).
- **CI.** `sanity` runs fmt and the layering check once; then `mac` and `linux` run clippy and the tests (`linux` also the bundle lint and tests), and `package-mac` and `package-linux` build the packages once their platform passed (the Linux job also runs `linux:smoke`).
- **Releases.** A `vX.Y.Z` tag runs `.github/workflows/release.yml`: `build-mac` runs `macos:package` universal (signed, notarized and stapled as the secrets allow) and `build-linux` runs `linux:package` on amd64 and arm64 (amd64 also `linux:smoke`). `publish` runs once every build passed and attaches `Recur-<v>-macos-universal.{dmg,zip}`, `Recur-<v>-linux-<arch>.{tar.gz,deb,rpm}`, unversioned latest-download aliases, `SHA256SUMS` and `manifest.json` to the private release and the public `recur-releases` mirror. The manifest is written and checked by `updater-manifest` (`crates/platform/src/bin`, Ed25519ph over each zip's SHA-512, verified with the same code the app runs after a download). `RECUR_UPDATE_URL` is `releases/latest/download/manifest.json` there. `mise run updater:verify <manifest>` checks a manifest by hand, and `mise run updater:genkey` writes a new key pair (rotating it means a new `build/updater.key.pub` and `UPDATER_SIGNING_KEY`, and installs on the old key stop accepting updates).

### Linux differences

- **HTML preview.** On macOS a WKWebView sits behind GPUI's view, which paints a transparent hole and a native hitbox over it (`vendor/README.md`, "Native views behind the drawing"), so overlays draw over the page and clicks there reach it; GTK 4 cannot embed a widget in a foreign (GPUI) window, so Linux runs an offscreen WebKitGTK page (`html_gtk.rs`), draws its snapshots and forwards input as DOM events. Those events are untrusted, so the shim mirrors `:hover` / `:active` rules onto attributes it sets on the pointer's chain and answers a `<select>` press with its options for a native menu; subframes are refused because WebKitGTK does not say which frame a navigation targets. A WPE WebKit 2.54 page with real input and frame rates sits behind `RECUR_HTML_WPE=1`; its tabled follow-up is `docs/linux-wpe.md`.
- **System UI font.** GPUI's Linux backend maps `.SystemUIFont` to IBM Plex Sans, so `recur_ui::fonts::system_family` asks fontconfig for `system-ui` instead, as WebKitGTK does; `Family::System` text uses that face.
- **Color panel** (project icon color): GTK's color chooser dialog, ported as `recur_ui::widgets::color_chooser` in its own window, where macOS opens `NSColorPanel` (`color_panel.rs`).
- **Notifications** go through `notify-send` with no click-through, and there is no dock badge.
- **Window icon** comes from the desktop entry, matched by the app id / `WM_CLASS` `build.recur.app`.
- **The updater** runs only in a macOS `.app`.
- **Backdrop blur.** GPUI's Linux renderer (gpui-pre-wgpu, not vendored) skips backdrop blurs, so the scrims, the maximized terminal dock and the creating-workspace overlay show their color alone (`vendor/README.md`).
- **Keys.** ⌘ chords are Ctrl (GPUI's `secondary`). Menu items carry no accelerators off macOS, and a focused terminal keeps its Ctrl chords (Ctrl+R, Ctrl+K, Ctrl+T…). The terminal dock's own chords are Ctrl+Shift, and CodeMirror's `Mod` bindings (Ctrl+G find next, Ctrl+End) take precedence over the emacs keymap. Specs press `secondary-…`; the few whose steps only hold on macOS are `ignore`d on Linux, each with the reason beside it.

