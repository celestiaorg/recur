# Recur

Recur is a desktop app for running coding agents on your own repositories. Each task gets an isolated git worktree, an agent of your choice, and a place to read the diff, review the result, and merge it back.

Recur is built with [Wails v3](https://v3.wails.io/) (Go backend) and Svelte 5 / TypeScript (frontend). It runs on macOS and Linux.

## What it does

- **Workspaces.** Every workspace is a git worktree on its own branch. Agents work in isolation, and you land their commits when you are satisfied.
- **Chats.** Run as many chats as you like at once, across workspaces or side by side in the same one. Each chat picks its own agent and one of five modes: Ask (read-only Q&A), Plan (read-only, produces a plan), Review (read-only, reports findings), Agent (read and edit), or Loop (runs a loop in the chat).
- **Loops.** In Loop mode, a chat hands the task to a team of specialist agents instead of one. A loop is a graph of phases, each run by a role with its own prompt and tools, that repeats until the result clears a bar you set. The shipped loops plan, implement, review, fix, and open a PR, but a loop can be any shape you declare. See [Loops](#loops) below.
- **Child workspaces.** An agent can spawn a child worktree for a subtask, then read, merge, or archive its result.
- **Review surface.** Diff viewer with inline comments and an approve flow, so review happens where the code is.
- **Editor.** A CodeMirror editor with TextMate grammars, themes, snippets, symbol outlines, and language servers, all supplied by bundles.
- **Providers.** Claude Code, Cursor, OpenAI Codex, and any OpenAI-compatible `/v1/responses` endpoint. See `docs/` for how each is wired.
- **Pull requests.** A PR dashboard across your projects that pulls in every open PR on each repo, not just the ones Recur opened. Review, comment, approve, fix checks, and merge without leaving the app. See [Pull requests](#pull-requests) below.
- **Schedules.** Save a prompt or loop to run on a cron cadence, once at a set time, or every time Recur starts. Each run gets its own fresh workspace. See [Schedules](#schedules) below.
- **Integrations.** Linear via MCP, with issue search and `/linear` straight from the composer. Recur's own MCP host tools let agents report findings, artifacts, and permission requests back to the app.

## Loops

A loop is a scripted collaboration between specialist agents that runs inside one workspace. You describe the task once, pick a loop and a policy, and the engine walks the loop's phases while you watch the chat, the diff, and the findings list.

**Roles.** Each specialist is a role: a system prompt, a chat mode, and an allow-list of tools. The shipped roles are Maestro, Planner, Coder, Reviewer, Security reviewer, QA, Writer, and Editor. The Reviewer can only read and file findings. The QA role has no file or shell access and drives a browser through Playwright to attach evidence. The Coder is the only role that edits.

**Phases.** A loop is a graph of phases, each of a built-in kind:

- **plan.** The Planner produces a structured plan. A policy gate decides whether the loop pauses for your approval before continuing.
- **converge.** The implementer does the work, then the reviewer files findings with severities, the implementer fixes or disputes them, and the reviewer re-reviews. The fix and re-review rounds repeat until open findings drop below the policy's threshold or the round cap is hit. A loop can also start a converge at the review step to work on a diff that already exists.
- **verify.** The QA role checks the result against the plan in a real browser and reports pass or fail with evidence.
- **ensure_pr.** Commits, pushes, and opens a GitHub pull request with an agent-written description, or updates the existing one.
- **artifact.** Publishes the result as a document instead of a PR.

Phases can be on demand. The To a PR loop has a security phase that only runs when the diff touches auth, secrets, or permission checks, or when you ask for it.

**Policies.** A policy sets the bar without changing the loop. It fixes the round cap, what happens when the cap is hit (escalate to you or proceed), which finding severities block completion, whether plans need approval, and a wall-clock timeout. The shipped policies are:

| Policy | Blocks on | Plan approval | Round cap |
|--------|-----------|---------------|-----------|
| Cautious | any finding | always | 2, then escalate |
| Balanced | medium and above | when the plan is structured | 4, then escalate |
| Aggressive | high and critical | never | 6, then proceed |

**The Maestro.** Every loop has a conductor. You talk to the Maestro, not the specialists. It reports where the run stands, edits the plan on your behalf, extends rounds when a converge escalates, re-enters a phase after a stop, and dispatches on-demand phases. It never does the phase work itself.

**Shipped loops.** Pick one from the loop picker, or type its slash command in any chat's composer to start it on that workspace.

| Loop | What it does | Slash command |
|------|--------------|---------------|
| To a PR | Planner writes a plan and waits for approval if the policy asks. Coder implements it, Reviewer and Coder converge on the diff, then the loop opens a pull request. Includes an on-demand security phase. | `/to-a-pr` |
| Review & fix | Starts at review on the diff already in the workspace. Reviewer files findings, Coder fixes them, until the bar passes. No PR. | `/review-and-fix` |
| Review & fix to a PR | Same as Review & fix, then opens or updates a pull request. | `/review-to-pr` |
| Security review & fix | Review & fix with the Security reviewer in place of the Reviewer, focused on auth, secrets, tokens, and permission checks. | `/security-review` |
| Docs | Planner drafts a documentation plan, Writer writes it, Editor reviews until no findings remain, and the result is published as an artifact instead of a PR. | `/write-docs` |

Loops, roles, phases, and policies are all declared in a bundle's `bundle.toml`. Copy the shipped `Loop` bundle to change a prompt, swap a role, or wire a new graph.

## Pull requests

Recur treats the pull request as the boundary where a human signs off. Everything an agent produces ends up in a diff you can read, comment on, and approve, whether or not it has been pushed to GitHub yet.

**Opening.** `/open-pr` and `/open-draft-pr` commit, push, and open a pull request with an agent-written title and summary. The ensure_pr phase in a loop does the same and updates the PR on later rounds. Recur talks to GitHub through the `gh` CLI, so it uses whatever account you have already authenticated.

**The dashboard.** The Pull requests view lists PRs across your projects, synced from GitHub. It includes PRs opened outside Recur, groups them into needs your attention, workspace is responding, open, and merged, and flags the ones that need you: a review was requested, you were assigned, changes were requested on your PR, or your checks are failing. Filter by project, status, review, or check state, and sort by updated, newest, oldest, or title.

**Reviewing.** Open any PR into the review surface. You get the file list, the full diff with syntax highlighting, commits, and the conversation. Leave line comments, batch them into a review, then submit it as a comment, an approval, or a request for changes. Reply to threads, resolve them, react, and mention people with autocomplete. Findings filed by reviewer agents show up alongside human comments.

**Acting.** From the PR titlebar you can mark a draft ready, merge with squash, rebase, or merge commit, and see how far the branch is behind its base. `/fix-checks` starts an agent on the failing checks. `/update` brings the branch up to date with its base. Any PR, including one opened elsewhere, can be checked out into a new workspace so an agent can work on it.

## Schedules

A schedule is a saved prompt that Recur runs on its own clock. Each occurrence creates a fresh workspace from the base branch you chose, starts the chat or loop, and records the outcome.

A schedule has a name, a prompt, a project and base branch, a provider model, and a mode. The mode is any of the five chat modes. Loop schedules also pick a loop and a policy, so a nightly "review and fix" or a weekly "security review" needs no one at the keyboard to start it.

Cadences:

- **Once** at a date and time in your time zone.
- **Cron** using standard five-field syntax or descriptors such as `@daily` and `@hourly`. Recur previews the next three run times before you save.
- **On startup** using `@reboot`, which runs every time Recur launches.

Schedules can be paused and resumed, or run immediately with Run now. If Recur was not running when an occurrence came due, the run is marked missed rather than replayed. If the previous run is still active or waiting on your input, the next occurrence is skipped. Deleting a schedule keeps the workspaces it already created.

## Bundles

Recur Bundles are TextMate-inspired packs that extend the app. Each bundle is a directory with a single `bundle.toml` and can ship any mix of:

- **Prompt actions.** Slash commands in the chat composer. The shipped `Chat` bundle provides `/commit`, `/commit-push`, `/open-pr`, `/open-draft-pr`, `/fix-checks`, `/review`, `/re-review`, `/fix-findings`, `/simplify-review`, `/simplify-audit`, and `/update`, plus `/to-a-pr`, `/review-and-fix`, `/review-to-pr`, `/security-review`, and `/write-docs`, which start the matching loop. Actions can take arguments with their own autocomplete.
- **Loops, roles, and policies.** Everything described under [Loops](#loops) comes from the shipped `Loop` bundle: the roles and their prompts, the phase graphs, and the three policies.
- **Palette search and task expansion.** The `Linear` bundle adds Linear issue search to the command palette and expands an issue reference into a full task prompt when you start a new workspace.
- **Editor commands.** Macros in the Run Command palette, bound to keys, or run on save.
- **Grammars, themes, terminal palettes, language data, snippets, symbols, and language servers.** Everything the editor needs to understand a language.
- **Settings.** Persisted values and secrets, such as API keys, that bundle scripts read at runtime.

The bundles under `bundles/` are the reference implementations. User bundles live in `~/.recur/bundles` and override shipped ones by id. Full authoring guide: [`docs/bundles.md`](docs/bundles.md).

## Installing

Download `Recur-<version>-macos-arm64.zip` from [Releases](../../releases), unzip it, and move `Recur.app` to `/Applications`. Releases are not notarized yet, so macOS blocks the first launch. Clear the quarantine flag once per download:

```bash
xattr -dr com.apple.quarantine /Applications/Recur.app
```

Downloading with `gh release download` skips this step, since the GitHub CLI does not set the flag.

On Linux (Ubuntu 24.04+ / Debian 13), install the GTK and WebKit runtime, then run `recur` from `Recur-<version>-linux-amd64.tar.gz` (or `-linux-arm64`):

```bash
sudo apt install libgtk-4-1 libwebkitgtk-6.0-4
tar xzf Recur-<version>-linux-amd64.tar.gz
./recur-linux-amd64/recur
```

## Command line

The app installs a `recur` command on your PATH at launch.

```bash
recur .                   # open the current folder
recur ./path/to/file      # open a file or folder
recur                     # focus Recur, launching it if needed
recur bundles lint ./bundles
recur bundles test ./bundles/JSON
```

## Building from source

Prerequisites:

- Go 1.25 or newer
- Node.js and [pnpm](https://pnpm.io/)
- The Wails v3 CLI: `go install github.com/wailsapp/wails/v3/cmd/wails3@latest`

All tasks run through `wails3 task` from the repository root.

```bash
wails3 task dev          # run the app with hot reload
wails3 task build        # build the desktop binary to bin/recur
wails3 task package      # produce a packaged app
wails3 task build:cli    # build the standalone CLI to bin/recur-cli
```

Set `RECUR_DEBUG=1` before `wails3 task package` to keep the web inspector and skip frontend minification.

Linux builds need `libgtk-4-dev` and `libwebkitgtk-6.0-dev`. `wails3 task linux:package` writes `bin/recur-linux-<arch>.tar.gz`; on a Mac it builds in Docker, and `wails3 task linux:run` opens the Linux app in a browser-based desktop.

## Testing

```bash
wails3 task check        # Go tests, bundle lint, bundle tests
wails3 task test         # everything above plus Playwright e2e
wails3 task test:e2e     # frontend e2e against a mocked Wails bridge
cd frontend && pnpm run check   # Svelte and TypeScript typecheck
```

The e2e suite drives the real UI in headless Chromium with the Go bridge mocked, so it needs no backend or agent installed. Details in [`frontend/e2e/README.md`](frontend/e2e/README.md).

## Repository layout

| Path | Role |
|------|------|
| `main.go`, `app_menu.go`, `window_*.go` | Desktop app entry and window chrome |
| `internal/` | Go packages: engine, store, MCP, bundles, CLI |
| `frontend/` | Svelte 5 + Vite UI |
| `bundles/` | Shipped Recur Bundles |
| `cmd/recur` | The `recur` CLI |
| `docs/` | Bundle authoring and provider docs |
| `Taskfile.yml`, `build/` | Build and packaging tasks |

## Contributing

Read [`AGENTS.md`](AGENTS.md) for conventions, quality gates, and PR guidance. Keep changes small and focused, run `wails3 task check` before pushing, and include e2e when you touch the UI.
