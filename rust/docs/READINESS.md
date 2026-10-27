# Recur Next readiness

Where the Rust/GPUI port stands against the Go/Svelte app, how that was checked, every known difference, and what to try by hand before switching day-to-day use to Recur Next. `PARITY.md` is the item-by-item record; this page summarizes it.

## Verdict

Every row of `PARITY.md` is ticked: each feature, behavior, persisted format and keyboard path of the Go/Svelte app has a Rust home with a test or a parity scene, or an explicit reason it does not apply. The differences that remain are the native-rendering allowances, the Linux platform differences and the deliberate changes listed below. The promotion checklist's remaining step is daily use.

## What is verified, and how

### The parity sweep (`wails3 task check:rust:parity`)

`rust/tools/parity-sweep/sweep.py` cross-checks `PARITY.md` against the sources, mechanically, on every `rust-mac` CI run:

| Check | What it proves | Result |
|---|---|---|
| bindings | every function in `frontend/bindings/recur/internal/app/*.ts` is a method of its `recur_services::api` trait and implemented by `AppServices` | 274 functions |
| events | every Wails event the frontend or Go emits or listens to is an `AppEvent` the real code builds or matches | 64 names (4 handled in process: the native menu and the titlebar double click) |
| tools | Go's agent tool catalog (dumped with `go run -overlay`) equals the golden file the Rust registry is tested against, in order, name, schema, description and halts | 42 host + 7 coding tools |
| cli | every cobra `Use:` in `internal/cli` is a `recur-cli` command | 6 commands |
| svelte | every `.svelte` file is a ticked `PARITY.md` row naming a Rust path that exists in a crate the desktop links; every view crate is registered | 94 components and views |
| gotests | every Go `func TestX` has a Rust test named `snake_case(X)` or a `go-tests.tsv` row naming the Rust test(s) that port it (checked to exist in that file) or why it does not apply | 2,640 Go tests: 2,636 ported, 4 not applicable |

The final audit used the sweep to find gaps rather than trust earlier ticks:

- **Go tests.** 238 Go tests had no counterpart the sweep could match. 190 were ported under other names and are now explicit rows. 48 had no Rust test; all were ported and every one passed against the existing code (none uncovered missing behavior): the Cursor context meter (10), transcript footers, pending and truncation (7), attachments and goal images (3), GitHub list parsing, check rollups and fork URLs (3), ask-user inbox rows (2), PR chat host hoists (2), the Cursor bridge pin notice (2), verify `on_reject`, send back, permission and per-role model wiring (6), usage parsing (4), and singles across the app, bundles, responses, compatible routing and loops. Four do not apply (Go slice aliasing, Go's raw-versus-mirroring runner split, a Go compile-time interface assertion, and a test Go skips on macOS and Linux).
- **Frontend e2e.** The remaining `shell.spec.ts`, `toasts.spec.ts` and `a11y-keys.spec.ts` scenarios were ported (usage meter popover, live PR count, group drag highlight, scrolling the selection into the tree, the escalated loop's inline question and its notes option, the project hide toggle, the three toast flows). Porting the hide toggle found a mock bug (an omitted `hidden: false` kept a project hidden in mock mode).
- **Terminal.** SGR 221 / 222 (kitty's not bold / not faint, on by default in xterm.js 6) now clear bold and faint alone.
- **Intel Macs.** WebM and Ogg (VP8, VP9, AV1, Theora, Opus, Vorbis) play on the Intel slice too: the codec crates build from source for x86_64 (`vendor/README.md`).

### Gate, packages and real data

Run in the foreground on an M-series Mac (macOS 26.3) on 2026-10-27, at the commits that close this audit:

| Step | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | clean |
| `cargo test --workspace --all-features --locked` | 5,650 passed, 0 failed, 10 ignored |
| `wails3 task check:rust:parity` | all six checks ok (table above) |
| `wails3 task test:rust:ui-parity` | 271 WebKit references captured, every scene within tolerance |
| `wails3 task linux:test:rust` (arm64 container) | fmt and clippy clean; 5,636 passed, 0 failed |
| `wails3 task linux:package:rust` | tarball, `.deb` and `.rpm` built |
| `wails3 task linux:smoke:rust` | passed (three runs in a row after the fix below) |
| `UNIVERSAL=true wails3 task package:rust` | `Recur Next.app` (arm64 + x86_64), DMG and zip; the Intel `recur-cli` runs under Rosetta |
| `wails3 task smoke:package:rust` | install, `recur-next` link and `recur-next .` pass; native notification delivery fails on this machine (below) |
| Real-data smoke (`real_data_smoke`, release), each range on a fresh `cp -Rc` of a copy of a real data dir (25 projects, 1,142 workspaces, 291 PRs), one at a time on a quiet machine | all four PASS: 0 panics, 0 error toasts, 0 stalls, 0 over budget |

The four ranges, run back to back with `RECUR_SMOKE_KEY_ROUNDS=4`:

| Range | Time | Worst per-field keystroke median |
|---|---|---|
| workspaces 1-285 | 703s | 6.7ms (commit message) |
| workspaces 286-570 | 125s | 3.8ms (commit message) |
| workspaces 571-855 | 207s | 5.1ms (line note) |
| workspaces 856-1142, every PR on each tab, Settings, Briefing, Issues, Schedules, palettes, a streamed turn | 2,884s | 7.4ms (PR comment) |

The streamed turn ran in a regular workspace, as it should: the earlier pick was a PR chat host that settles and archives once idle, as in Go.

Two fixes came out of this run:

- **The boot notice could be dropped.** The desktop subscribed to app events inside a spawned task, so `start()` could publish the Cursor bridge notice first (one `linux:smoke:rust` run in two missed it). It now subscribes before boot publishes.
- **`UNIVERSAL=true package:rust` failed** because the script ran cargo from the repo root, where rustup does not see `rust/rust-toolchain.toml`, so the Intel codec build had no `llvm-tools`. It now builds from `rust/`.

`smoke:package:rust`'s notification check fails here with and without those changes, ad-hoc or signed with a local identity: usernoted logs `Failed to find or validate client of identifier build.recur.next` for every launch and refuses the authorization request, so the app falls back to `osascript`, which usernoted delivers. The notification reaches the user; the native path needs a check on a second Mac (try by hand, item 5).

## Known differences from the Go app

### Native-rendering allowances (`ui-parity.md`)

The bar since 2026-10-28: same look and feel, minor differences fine, never trade speed for pixels. Every parity scene passes its tolerance; these are the recorded differences:

- **Intentional differences** (limits of GPUI or of matching a browser's private rendering): gradient dither, glyph subpixel phase, curved-edge coverage, no backdrop blur (palette, modal and lightbox scrims keep their color), no element transforms (the toast entrance drops its 0.98 scale), carets drawn by the editor, upright text where WebKit synthesizes oblique code italics, synthetic-italic edge pixels, frosted terminal cells over the maximized dock, sticky headers as absolute layers, a collapsing rail that stays hit-testable for its 160ms slide, the overlay tab-strip scroller, a drawn native checkbox in task lists, image resampling, and LSP documentation links (a full-cyan underline for the 45% cyan border) and wrapped signatures (no hanging indent).
- **Hard-pixel allowances**: a handful of scenes allow a few device pixels (rounded corners at fractional offsets, typed field text, the vim panel's Courier weight, a classic scrollbar thumb a pixel shorter across a rounding boundary). Listed with causes in `ui-parity.md`.
- **Closed under the bar**: a long wrapped file's scroll thumb can be off by a pixel until its lines are measured; the Observatory at 640 by 600 stretches the editor where WebKit leaves 74px empty; a bottom-pinned transcript can sit a fraction of a pixel lower.

### Linux

From `ARCHITECTURE.md` "Linux differences": the HTML preview is an offscreen WebKitGTK page drawn as snapshots with input forwarded as DOM events (a shim mirrors `:hover` / `:active` and pops `<select>` menus natively; subframes are refused); the system UI font comes from fontconfig's `system-ui`; Mermaid measures at the first monitor's integer scale; the icon color picker is a GTK-style chooser window; notifications go through `notify-send` with no click-through and no dock badge (as in the Go app); the window icon comes from the desktop entry; the updater runs only in a macOS `.app` (as in the Go app); ⌘ chords are Ctrl, and menu items carry no accelerators. Date and time fields use the en-US pattern off macOS.

### Deliberate changes

- **Side by side.** Recur Next has its own data dir (`RecurNext`), worktrees (`~/.recur/worktrees-next`), identity, bundle id, open-IPC socket and CLI link (`recur-next`), and refuses a database with migrations it does not know. Pointing it at a copy of real data is `RECUR_DATA_DIR`.
- **Grep in a dot-named root.** Search, replace and the symbol walk search a worktree whose root is dot-named (`/tmp/.tmpXXXX`, `~/.dotfiles`); Go finds nothing there.
- **Theora on macOS.** Ogg Theora plays on both Mac slices through `theora-rs`, where WebKit shows the poster.
- **No HTML from agents or PRs in a webview.** Markdown is sanitized with the same DOMPurify rules and laid out natively; images load only from `data:` and the GitHub image proxy.
- **Accessibility test.** Sidebar icon buttons carry the same accessible names (through AccessKit), but GPUI's test window has no AccessKit adapter, so that e2e check is manual (below).
- **A change to the open PR redraws its conversation once** (in its own frame, as Svelte's keyed blocks re-run); keys still render the field alone and stay inside the 20ms budget.

## Promotion checklist (`ARCHITECTURE.md` "Promotion")

| Step | Status |
|---|---|
| `PARITY.md` fully ticked | Done (this audit) |
| `check:rust:parity`, lint and tests in CI (`rust-mac`, `rust-linux`), packages built (`package-rust-mac`, `package-rust-linux`) | Done |
| Release workflow builds Recur Next beside the Go app | Wired and checked with `actionlint` and a locally signed manifest; not yet run on a tag |
| Recur Next as the daily driver "for a while" | Not started: the remaining gate |
| 1. Delete `internal/`, `cmd/`, root `*.go`, `go.mod`, `frontend/`, Wails build tasks and Go CI jobs | After daily use. The parity sweep's tools and bindings checks read Go and Svelte sources and retire with them |
| 2. Move `rust/*` to the repo root, merge `rust/build/` into `build/`, update `include_dir!` paths for `bundles/`, `recur-skills/`, migrations and assets | After step 1 |
| 3. Switch the data dir back to `Recur`, worktrees to `~/.recur/worktrees`, identity to `Recur`, the CLI link to `recur` | After step 2; the store's schema guard already accepts the Go app's database |

## Try by hand before switching

The specs and scenes cover behavior and look; these need a person, real accounts or real hardware:

1. **Your real data.** Run Recur Next on a copy of your data dir (`RECUR_DATA_DIR=<copy>`), not the original: open a few large workspaces, the longest chat, a big review, PR detail on a busy PR, Settings, Briefing, Issues and Schedules. Quit and relaunch: the last view and per-view state come back.
2. **Real agent turns** with each provider you use (Claude Code, Codex, Cursor, a compatible endpoint): a turn with tool calls and a permission prompt, Stop mid-turn, steer while running, switch modes mid-turn, a Loop run through to an opened PR, and an `ask_user` question answered from the inbox.
3. **GitHub.** Sync PRs, comment, reply on a line, react, approve, request changes, merge (and a failed merge's toast), Fix checks, and create a workspace from a PR.
4. **Typing feel.** Type fast in the chat composer, a commit message, a line note and a PR comment on your largest workspace; scroll a long transcript and a big review. The budget is 20ms a key; anything you can feel is a bug.
5. **Native integration.** Drag files from Finder onto the chat and the terminal; the menu bar and its accelerators; the dock badge; a notification shown as Recur Next (not Script Editor) and its click; `recur-next .` from a terminal; dark / light system switching; full screen; a second monitor at a different scale.
6. **VoiceOver.** The sidebar's icon buttons (Settings, Get started, Add project) and the chat composer read their names.
7. **An Intel Mac** (or Rosetta): open a WebM and an Ogg attachment in the Observatory and the lightbox.
8. **Linux** (Debian 13 / Ubuntu 24.04+, `.deb` or tarball): the HTML preview (hover, a `<select>`, typing), Mermaid and PDF previews, notifications, and Ctrl chords in the terminal.
9. **The updater**, once `RECUR_NEXT_UPDATE_URL` points at a published `manifest-next.json`.
