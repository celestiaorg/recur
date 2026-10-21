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
    app/                recur-app          windows and views         (App.svelte, views/, components/)
    desktop/            bin `recur`        desktop entry point       (main.go and root *.go)
    cli/                bin `recur-cli`    command line              (cmd/recur, internal/cli)
  providers/
    claude/ codex/ cursor/ compatible/     one crate per agent provider
  docs/
```

Remaining Go packages map as follows: `observe` and `simplify` live as modules of `recur-engine` (or `recur-code` for pure file logic) unless their consumers show a better home; record the final home in `PARITY.md`.

When `recur-app` grows past comfortable compile times, split views into crates under `crates/views/<area>` (`chat`, `review`, `prs`, `observatory`, `settings`, `schedules`, `briefing`, `palettes`) that depend on `recur-ui` and `recur-services` only. `crates/*` globbing picks them up only if they sit directly under `crates/`, so add `crates/views/*` to `members` when the first one lands.

## Layering

Dependencies point down only:

```
desktop, cli
app ──────────────► editor, terminal-view, ui
services ─────────► engine, providers/*, forge, issues, usage, lsp, platform
engine ───────────► agent, agent-tools, mcp, bundles, code, git, store, term
agent, forge, issues, bundles, git, code, store, project ─► domain
textmate, term, ui ─ standalone
```

Rules:

- `recur-domain` has no I/O and no async.
- Nothing below `recur-services` knows about GPUI. Nothing below `recur-app` renders.
- `recur-engine` depends on the `recur-agent::Provider`, `recur-forge::Forge` and `recur-issues::IssueTracker` traits, never on a concrete implementation. `recur-services` is the composition root that registers the shipped providers, forges and trackers.

## Extension points

Adding a provider, forge or issue tracker means adding a crate (providers) or a module (forges, trackers) and one registration line in `recur-services`:

- **Agent providers** implement `recur_agent::Provider` (capabilities, model catalog, flags, start/resume/steer/cancel, normalized event stream, usage). The engine only talks to the trait.
- **Remote git hosting** implements `recur_forge::Forge` (detect from remote URL, list/sync PRs, reviews, threads, reactions, checks, merge, branch rules, mentions, images). GitHub (via `gh`) ships first.
- **Issue trackers** implement `recur_issues::IssueTracker` (search, fetch, expand to task prompt, workspace linking).
- **Bundles** keep their existing on-disk format and semantics; `recur-bundles` is a faithful port.

Registries are keyed by the same string ids the Go app persists (`claude`, `codex`, `cursor`, `compatible:<id>`, `github`, `linear`, ...), so stored data stays valid.

## Runtime model

- The backend (`engine` and below, `services`) runs on a shared multi-threaded `tokio` runtime: subprocess I/O, sockets, HTTP, timers and file watching. Go goroutines become tasks; `context.Context` cancellation becomes `tokio_util::sync::CancellationToken`; Go channels become `tokio::sync` channels.
- The UI runs on GPUI's foreground executor. `recur-services` exposes async methods (the former Wails-bound methods, same names in snake_case) and a typed `AppEvent` broadcast that replaces the Wails event names. `recur-app` bridges events into GPUI entities with `cx.spawn` and never blocks the foreground thread.
- The Go event bus (`engine/bus.go`) becomes a typed broadcast in `recur-engine`; services translate it to `AppEvent`.

## Conventions

- Edition 2024, toolchain pinned in `rust-toolchain.toml`, workspace lints in `Cargo.toml`. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` stay clean.
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
