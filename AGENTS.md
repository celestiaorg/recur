# AGENTS.md

Guidance for coding agents working in this repo. Prefer the Taskfile over ad-hoc scripts.

## What this is

Recur is a Wails v3 desktop app (Go backend + Svelte 5 / TypeScript frontend) with an optional authoring CLI and TextMate-inspired **Recur Bundles** under `bundles/`.

Read before inventing architecture:

- `docs/bundles.md` — bundle authoring, keybindings, CLI
- `docs/` — living provider docs

## Layout

| Path | Role |
|------|------|
| `main.go`, `app_menu.go`, `window_state.go` | Desktop app entry / chrome |
| `internal/` | Go packages (engine, store, mcp, bundles, cli, …) |
| `frontend/` | Svelte 5 + Vite UI |
| `bundles/` | Shipped Recur Bundles (reference implementations) |
| `cmd/recur` | CLI (`recur .` opens a path; `recur bundles …` authors bundles) |
| `Taskfile.yml` | Root tasks (`wails3 task …`) |
| `build/` | Wails build tasks / platform packaging |

## Dev environment tips

- Orchestrator is **Task via `wails3 task`** (root `Taskfile.yml`). go-task is not required separately.
- Task names use **`verb:object`** (`test:bundles`, `lint:bundles`, `build:cli`).
- Dev app: `wails3 task dev` (or `wails3 task run` / `wails3 task build`).
- Dogfood: `.recur/project.toml` sets `RECUR_INSTANCE_LABEL=$RECUR_BRANCH`. When that env is present, Recur paints a `dev` chip with the branch in the titlebar gap (packaged Recur omits it).
- Packaged debug build: `RECUR_DEBUG=1 wails3 task package` keeps the web inspector and skips frontend minification (sourcemaps included).
- Linux: `wails3 task linux:build` / `linux:package` (GTK4 + WebKitGTK 6.0, so Debian 13 / Ubuntu 24.04+). On a Mac it builds inside `build/linux/Dockerfile` (Docker or OrbStack) for the host arch into `bin/recur-linux-<arch>/`, never `bin/recur`. The package is `bin/recur-linux-<arch>.tar.gz` (`recur` + `recur-cli`); running `recur` from it links `~/.local/bin/recur` to the CLI and writes an app-menu entry (`~/.local/share/applications/org.wails.recur.desktop`).
- Try the Linux app from a Mac: `wails3 task linux:run` builds it and opens a throwaway Linux desktop (Xvfb + noVNC) in the browser at `http://127.0.0.1:6080`. Its home persists in the `recur-linux-home-<arch>` Docker volume; Ctrl+C or File > Quit stops it.
- Desktop binary: `bin/recur`. Dev CLI: `bin/recur-cli` via `wails3 task build:cli` — do not overwrite one with the other. Packaged apps also embed `Contents/MacOS/recur-cli` (same `./cmd/recur` binary) and symlink it onto PATH at launch (D-cli-install).
- Frontend deps live in `frontend/` (`pnpm` / `pnpm-lock.yaml` there). Typecheck: `cd frontend && pnpm run check`.
- After changing Go methods exposed to the UI, regenerate bindings through the Wails build path (committed `.ts` bindings are authoritative).
- UI view state that should survive view switches or reloads (filters, sort, layout, expanded rows) is persisted on disk, never `localStorage`/`sessionStorage`. Add a JSON file under `store.DataDir()` via `ShellService` in `internal/app/shell.go` (see `shell.json`, `schedule-view.json`, `issues-view.json`, written with `writeDataJSON`). Webview storage is wiped on dev↔packaged origin changes.
- Bundles need `node` on `PATH` for TypeScript macros and engine tests.
- Multi-agent coordination (optional): gitignored `TODO.md` — claim work there so sessions do not collide.

## Testing instructions

CI: `.github/workflows/ci.yml` runs Go lint + govulncheck + tests on macOS (`go-mac`) and Go lint + tests on Ubuntu 24.04 (`go-linux`), `-race` on Ubuntu (`go-race`), bundles, frontend, and e2e as separate jobs, and packages Recur.app and the Linux tarball (`package-mac`, `package-linux`), failing if committed bindings or `go.mod` are stale. Branch protection requires only its `CI success` job, so add new checks as jobs there and list them in `ci-success.needs`. Local quality gates are Taskfile-driven.

| Task | What |
|------|------|
| `wails3 task check` | Fast gate: Go tests, `lint:go`, `lint:frontend`, bundle lint/tests, frontend unit (no Playwright) |
| `wails3 task check:cursor-sdk-pin` | Network: fail if PyPI `cursor-sdk` is ahead of `pinnedSDKVersion` |
| `wails3 task check:vulns` | Network: govulncheck on reachable Go code |
| `wails3 task test` | Everything in `check` plus `test:e2e` |
| `wails3 task test:go` | `go test ./...` |
| `wails3 task test:go:race` | `go test -race ./...` (slow) |
| `wails3 task lint:go` | `go vet`, `gofmt`, staticcheck |
| `wails3 task lint:frontend` | svelte-check |
| `wails3 task lint:bundles` | Schema, TS compile, shebang, keyEquivalent collisions |
| `wails3 task test:bundles` | Shipped bundle engine tests under `./bundles` |
| `wails3 task test:e2e` | Frontend Playwright (mocked Wails bridge) |
| `wails3 task build:cli` | Build `bin/recur-cli` |

### Cursor SDK pin

Recur pins `cursor-sdk` (`pinnedSDKVersion`) so the attribution minify patch stays valid. A weekly GitHub Action (`.github/workflows/cursor-sdk-pin.yml`) and `wails3 task check:cursor-sdk-pin` compare the pin to PyPI latest. On drift, follow `.agents/skills/repin-cursor-sdk/SKILL.md` (bump pin + refresh `sdkAttributionAnchors` together). Details: `docs/cursor-provider.md`.

Focused runs:

```bash
go test ./internal/bundles/...
go test -run TestName ./internal/cli/...
cd frontend && pnpm run e2e          # or pnpm run e2e:ui
bin/recur-cli bundles test ./bundles/JSON
bin/recur-cli bundles lint ./bundles
```

Expectations:

- Before merging non-trivial changes, `wails3 task check` should be green; run `wails3 task test` (or at least `test:e2e`) when touching UI.
- Add or update tests for the code you change.
- Bundle changes: keep `lint:bundles` clean (errors fail; shared keyEquivalents may log info).
- After store seed / domain shape changes that affect e2e fixtures: `cd frontend && pnpm run e2e:seed`.

## Scope

- Solve the stated task with the smallest change that does the job. Do not widen scope into speculative robustness work (extra platforms, hypothetical edge cases, new abstractions) unless asked.
- Review findings about hypothetical cases are optional follow-ups to mention, not requirements to implement. Successive review rounds must not balloon a small fix.
- If a branch's diff looks disproportionate to the ask, reset to HEAD and re-derive the minimal fix rather than trimming incrementally.

## Voice

- Do not use em dashes (U+2014). Prefer a period, comma, colon, or hyphen.

## PR instructions

- Prefer small, focused PRs; match existing commit tone (why over noise).
- Run `wails3 task check` before pushing; include e2e when frontend behavior changes.
- Update `docs/bundles.md` for author-facing bundle behavior.
- Do not commit secrets, `bin/`, Vite output under `frontend/dist/` (keep `.gitkeep`), or the gitignored `TODO.md`.
