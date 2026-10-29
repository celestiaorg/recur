# AGENTS.md

Guidance for coding agents working in this repo. Prefer `mise run <task>` over ad-hoc scripts.

## What this is

Recur is a native desktop app written in Rust on GPUI (through gpui-kit), with an authoring CLI and TextMate-inspired **Recur Bundles** under `bundles/`.

Read before inventing architecture:

- `docs/ARCHITECTURE.md`: crates, layering, view crates, services contract, packaging
- `docs/bundles.md`: bundle authoring, keybindings, CLI
- `docs/`: provider docs, `ui-rendering.md` (how recur-ui draws), `FOLLOWUPS.md` (open items)

## Layout

| Path | Role |
|------|------|
| `Cargo.toml`, `rust-toolchain.toml` | Cargo workspace and the pinned toolchain |
| `crates/` | Workspace crates (domain, store, engine, services, ui, views, desktop, cli, …) |
| `providers/` | One crate per agent provider (claude, codex, cursor, compatible, chatgpt) |
| `vendor/` | Patched crates.io sources (gpui-pre and friends; see `vendor/README.md`) |
| `tools/layering` | The crate layering check (`mise run lint`) |
| `bundles/` | Shipped Recur Bundles (reference implementations), embedded by `recur-bundles` |
| `recur-skills/` | Shipped agent skills, embedded by `recur-bundles` |
| `build/` | Packaging: macOS app/DMG (`build/darwin`), Linux Docker images and packages (`build/linux`) |
| `mise.toml`, `tasks/` | Tools (node, pnpm, nfpm, dmgbuild) in `mise.toml`; tasks in `tasks/check.toml`, `dev.toml`, `macos.toml`, `linux.toml`, `release.toml` |
| `src-web/` | The website; `src-web/demo` is the Svelte app its interactive demo builds |
| `src-docs/` | User docs (also copied into the `recur-help` skill) |

## Dev environment tips

- Install [mise](https://mise.jdx.dev) and run `mise install` once; Rust comes from `rust-toolchain.toml` through rustup. `mise tasks` lists every task.
- Task names use **`verb:object`** (`test:bundles`, `lint:bundles`, `linux:package`).
- Dev app: `mise run run` (`cargo run -p recur-desktop`). It defaults to the `Recur` data dir, the same as the packaged app; `RECUR_DATA_DIR` / `RECUR_WORKTREES_DIR` override it.
- Dogfood: `.recur/project.toml` sets `RECUR_INSTANCE_LABEL=$RECUR_BRANCH`. When that env is present, Recur paints a `dev` chip with the branch in the titlebar gap (packaged Recur omits it).
- Everything builds into `target/`: `target/debug/recur` (desktop) and `target/debug/recur-cli` (CLI). Packages go to `target/package/`.
- macOS package: `mise run macos:package` (`Recur.app`, DMG and updater zip; `UNIVERSAL=true`, signing and notarization env in `build/darwin/package.sh`). `mise run macos:smoke` installs and launches the DMG in a sandbox.
- Linux: `mise run linux:test`, `linux:build`, `linux:package` (tarball, `.deb`, `.rpm` to `/opt/recur`) and `linux:smoke`, all in `build/linux/Dockerfile` on a Mac (Docker or OrbStack) for the host arch (`ARCH=amd64|arm64` overrides). `mise run linux:run` opens the Linux app on a throwaway desktop at `http://127.0.0.1:6081` (noVNC). `linux:test:wpe` runs the WPE WebKit spike's checks.
- UI view state that should survive view switches or restarts (filters, sort, layout, expanded rows) is persisted on disk as a JSON file in the data dir, through the shell service in `crates/services/src/app/shell.rs` (see `shell.json`, `schedule-view.json`, `issues-view.json`).
- Bundles need `node` on `PATH` for TypeScript macros and engine tests.
- Multi-agent coordination (optional): gitignored `TODO.md`: claim work there so sessions do not collide.

## Testing instructions

CI: `.github/workflows/ci.yml` runs `quick` first (fmt and the layering check, once), then clippy and the workspace tests on macOS (`mac`) and Linux (`linux`, all features under Xvfb, plus bundle lint and tests), and packages each platform only after its tests pass (`package-mac`, `package-linux` with the Debian smoke test). Branch protection requires only its `CI success` job, so add new checks as jobs there and list them in `ci-success.needs`. Local quality gates are mise tasks.

| Task | What |
|------|------|
| `mise run check` | Gate before merging: `lint`, `test`, `lint:bundles`, `test:bundles` |
| `mise run lint` | `cargo fmt --check`, clippy (`-D warnings`), the layering check |
| `mise run test` | `cargo test --workspace` |
| `mise run lint:bundles` | Schema, TS compile, shebang, keyEquivalent collisions (Rust `recur-cli`) |
| `mise run test:bundles` | Shipped bundle engine tests under `./bundles` |
| `mise run check:cursor-sdk-pin` | Network: fail if PyPI `cursor-sdk` is ahead of `PINNED_SDK_VERSION` |
| `mise run linux:test` | fmt, clippy and tests on Linux (Docker on a Mac) |
| `mise run test:splash` | Builds the website and browser-checks the demo |

### Cursor SDK pin

Recur pins `cursor-sdk` (`PINNED_SDK_VERSION` in `providers/cursor/src/install.rs`) so the attribution minify patch stays valid. A weekly GitHub Action (`.github/workflows/cursor-sdk-pin.yml`) and `mise run check:cursor-sdk-pin` compare the pin to PyPI latest. On drift, follow `.agents/skills/repin-cursor-sdk/SKILL.md` (bump the pin and the attribution anchors together). Details: `docs/cursor-provider.md`.

Focused runs:

```bash
cargo test -p recur-bundles
cargo test -p recur-cli --test cli
cargo run -q -p recur-cli -- bundles test ./bundles/JSON
cargo run -q -p recur-cli -- bundles lint ./bundles
```

Expectations:

- Before merging non-trivial changes, `mise run check` should be green.
- Add or update tests for the code you change.
- Bundle changes: keep `lint:bundles` clean (errors fail; shared keyEquivalents may log info).
- After store seed / domain shape changes: regenerate the mock seed with `cargo run -p recur-store --bin seeddump > crates/services/src/mock/seed.json`.

## Scope

- Solve the stated task with the smallest change that does the job. Do not widen scope into speculative robustness work (extra platforms, hypothetical edge cases, new abstractions) unless asked.
- Review findings about hypothetical cases are optional follow-ups to mention, not requirements to implement. Successive review rounds must not balloon a small fix.
- If a branch's diff looks disproportionate to the ask, reset to HEAD and re-derive the minimal fix rather than trimming incrementally.

## Voice

- Do not use em dashes (U+2014). Prefer a period, comma, colon, or hyphen.

## PR instructions

- Prefer small, focused PRs; match existing commit tone (why over noise).
- Run `mise run check` before pushing.
- Update `docs/bundles.md` for author-facing bundle behavior.
- Do not commit secrets, `target/`, `src-web/public/demo/`, or the gitignored `TODO.md`.
