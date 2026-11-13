---
name: project-toml
description: >-
  Create or edit a Recur project's .recur/project.toml (run settings). Use when
  the user wants setup, teardown, named runs, the Run button, app URL, env, or
  files copied into new workspaces; or to author, scaffold, or fix project.toml.
---
# Recur project.toml

Run settings live in **one file**: `<repo>/.recur/project.toml`. Recur reads it
on every project load. Settings → Run writes it. It is meant to be committed.

Machine-local fields (repo path, sidebar, pinned Run, skills, merge method, GitHub
PRs) stay in Recur's DB. Do not invent keys for those.

## Where to write

The git checkout of the project whose workspaces should pick this up: usually
the current workspace root. Not `~/.recur/`, not Recur's data dir.

If the file already exists, **read it first** and patch in place (keep comments).
A missing file is empty run-config. A malformed file is also treated as empty;
fix the TOML rather than relying on the settings page to rewrite it.

## Workflow

1. Open `.recur/project.toml` (create `.recur/` if needed).
2. Change only the field the user asked for. Copy shape from an existing
   `[[run]]` or the skeleton below: do not invent keys.
3. Dev servers must bind `$RECUR_PORT`. Loops that verify-in-browser look for a
   run named `dev`, else the first `[[run]]`. No bootable run → no verify → no PR.
4. Secrets go in gitignored dotenv, not `env`. Confirm `.gitignore` has
   `.env`, `.env.local`, `.recur/.env`, `.recur/.env.local`.

## Fields

| Key | Shape | When it runs |
|-----|--------|----------------|
| `setup_cmd` | bash (`/bin/sh -c`) | Once on worktree **create**, after copy-files. Cwd = worktree. 10 min. Failure fails create; stderr/stdout tail is shown on the workspace and a hand workspace can Retry setup after fixing the command. |
| `teardown_cmd` | bash | Once on remove/delete, **best-effort**. Cwd = worktree (never the real checkout if that path is empty). 10 min. |
| `[[run]]` | `name` + `cmd` | Titlebar Run. Empty `cmd` is skipped. Empty `name` → `run`. Duplicate names get `-2`. |
| `app_url` | URL | Verify-in-browser probe. `$RECUR_PORT` / `$PORT` substituted. Empty → `http://localhost:$RECUR_PORT`. |
| `env` | dotenv text | Injected into setup, teardown, runs, Observatory. `KEY=value`, `#` comments, optional `export `. |
| `copy_files` | globs, one per line | Copied from the project checkout into each **new** worktree, **before** setup. Existing dest files skipped. Gitignored files are included (that is the point). |
| `conventional_commits` | bool | `true`: `/commit`, `/open-pr`, generated commit messages and loop PR titles use Conventional Commits (`type(scope): summary`). |

`#` comments and blank lines in `copy_files` / `env` are ignored. `copy_files`
does not support `!` negation. A pattern with no slash matches at any depth
(`.env*` → `apps/web/.env.local`). Trailing slash = that directory. Heavy dirs
(`node_modules`, `dist`, `build`, …) are skipped unless the pattern names them.
Cap: 10 000 files, 32 MiB each. A pattern ending in `/` (`node_modules/`,
`target/`) copies each matching directory whole, symlinks included and past the
caps: APFS clones on macOS (fast, no extra disk), a reflink on
btrfs/XFS, else a full copy. Good for warm dependencies before `setup_cmd`.

Do not write `pinned_run`: that default stays on this machine, not in the file.

## Env merge (later wins)

host → Recur vars → `project.toml` `env`. Dotenv files are **not** exported.
They may still be referenced when expanding `project.toml` `env` values
(`AUTH=Bearer $API_KEY`).

`project.toml` `env` **cannot** set `PATH`, `HOME`, `SHELL`, `LD_*`,
`DYLD_*`. Unknown `$FOO` stays as literal text.

Recur injects: `$RECUR_PORT` / `$PORT`, `$RECUR_WORKSPACE_ID`, `$RECUR_WORKTREE`,
`$RECUR_ROOT` (the project's main checkout), `$RECUR_PROJECT_ID`,
`$RECUR_BRANCH` (empty string if unset). `$HOME` works.

## Skeleton

```toml
# Recur project settings: setup, teardown, named runs, app URL, env, and files to copy.
# Safe to commit and share. Keep secrets in gitignored dotenv for the app to load;
# Recur copies them via copy_files and does not export them.

setup_cmd = """
pnpm install
"""

teardown_cmd = ""
app_url = "http://localhost:$RECUR_PORT"

env = """
# NODE_ENV=development
"""

copy_files = """
# .env*
# **/.env.local
"""

[[run]]
name = "dev"
cmd = "pnpm dev --port $RECUR_PORT"

[[run]]
name = "test"
cmd = "pnpm test"
```

Multiline strings use `"""…"""`. `[[run]]` is an array of tables: never `[run.0]`.

Skills (`SKILL.md`) and Bundles (`bundle.toml`) are different: `/create-skill`
and `create-bundle`.
