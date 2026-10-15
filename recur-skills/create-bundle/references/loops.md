# Loops, roles, and policies

Read this when the user wants a custom Loop (or a role / policy for one).

## Start from a shipped shape

| Ask | Copy |
|-----|------|
| Plan → code → PR | `recur.loops.pr` (To a PR) |
| Review the diff I already wrote | `recur.loops.converge` — `entry = "converge"`, `start = "review"`, no `ensure_pr` |
| Review that diff, then PR | `recur.loops.converge-pr` — same as converge, then `ensure_pr` |
| Write docs, no PR | `recur.loops.docs` — custom Writer/Editor roles, `findings_threshold = 0`, terminal `uses = "artifact"` |
| Security look at this diff | `recur.loops.security` — `start = "review"`, reviewer `recur.roles.security-reviewer` |

Prefer **reuse** `recur.roles.*` and the three shipped policies. Mint a new
role only when the persona or tool set is actually different (Docs is the
teaching example). Mint a policy only when Cautious / Balanced / Aggressive
is the wrong bar.

When this worktree is Recur, read `bundles/Loop/bundle.toml` and
`bundles/Loop/prompts/`. Elsewhere, treat those ids as the contract and
write a user pack under `~/.recur/bundles`.

## Pack skeleton (loop-only)

```toml
[bundle]
name = "Acme Loops"
id = "acme.loops"
version = "1.0.0"
author = "You"

[[loop]]
id = "acme.loops.review"
name = "Review this"
hint = "Review and fix the current diff — no PR"
conductor = "recur.roles.maestro"
prompt = "prompts/mission.md"
entry = "converge"
reenter = ["converge"]

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
on_max_rounds = "escalate"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[loop.phase.prompts]
review = "/review"
fix = "/fix-findings"
rereview = "/re-review"
```

Ids: filename-safe, ≤80 chars, letters/digits/`_`/`-`/`.`, no `/` or `..`.
Convention: `you.loops.<name>`, `you.roles.<name>`, `you.policies.<name>`.

## `[[role]]`

```toml
[[role]]
id = "acme.roles.writer"
name = "Writer"
code = "WR"              # 2 letters, unique in the discovered catalog
mode = "agent"           # required: ask | plan | agent | review | loop
# tools = ["update_plan_step"]
prompt = "prompts/writer.md"
```

- `mode` is required. It picks tools and readonly vs edit.
- Only `mode = "loop"` may fill `roles.conductor`. Use `recur.roles.maestro`
  unless you have a real reason not to.
- Slot modes: planner → `plan`; implementer → `agent`; reviewer → `review`;
  verifier → `ask` / `review` / `agent`.
- `tools` are short names (`update_plan_step`, `report_finding`,
  `playwright`, …). Do not write `mcp__recur__*` in the manifest.
- `disabled_tools` can subtract. Cannot add Edit onto a readonly mode.
- Role prompt: identity + severity calibration + tool literacy. Keep it short.

## `[[policy]]`

How hard the run pushes. The loop is the graph; the dock policy is the bar.
Authors who want a different bar write a policy — they do not punch rails.

| | Cautious | Balanced | Aggressive |
|---|---|---|---|
| `max_rounds` | 2 | 4 | 6 |
| `on_max_rounds` | escalate | escalate | proceed |
| `findings_threshold` | all five = 0 | critical/high/medium = 0 | critical/high = 0 |
| `plan_approval` | always | if_structured | never |
| `extend_rounds` | 1 | 2 | 3 |

Phase-level `max_rounds` / `on_max_rounds` / `findings_threshold` /
`timeout` / `retries` / `on_fail` override that phase only. A phase
findings map must be **same-or-stricter** than the selected policy.
Integer `findings_threshold = 0` means all five severities at 0 (ok).
Integer `N > 0` is a lint error.

## `[[loop]]` graph

Closed `uses` verbs and their slots:

| `uses` | Role slots | Prompt slots |
|--------|------------|--------------|
| (loop) | **conductor** (`loop`) | mission (opt.; `prompt =` sugar) |
| `plan` | **planner** (`plan`) | plan |
| `implement` | **implementer** (`agent`) | implement |
| `review` | **reviewer** (`review`) | review |
| `converge` | **implementer** + **reviewer** | implement, review, fix, rereview |
| `verify` | **verifier** | verify |
| `ensure_pr` | — | — |
| `artifact` | — | — |

Bold slots are required. Scalar `role =` / `prompt =` is legal only when
that kind has exactly one slot of that kind. Otherwise use
`[loop.phase.roles]` / `[loop.phase.prompts]`.

- `entry` must exist. `next` is a graph edge. **No cycles.**
- `reenter` is graph ids only. `dispatch = "on_demand"` phases cannot be
  in `reenter` (conductor `run_phase` only).
- `ensure_pr` and `artifact` are terminals: no specialist spawn. Empty
  `next` idles. `artifact` just parks; the write-up lives in the worktree.
- Converge `start = "review"` skips the first implement (human already
  wrote the diff).
- `gate = "plan_approval"` on `plan` honors the **policy**
  `plan_approval` value.
- `when` on on-demand phases is conductor guidance, not an engine trigger.
- Prompt values: a composer slash (`/review`), a pack-relative path
  (`prompts/security-review.md`), or inline text (anything with a space,
  or without a `/` and not ending in `.md`). Paths must exist. Do not
  write `prompt = { a, b }`.
- A phase prompt replaces the engine's built-in turn text for that slot.
  The built-in `fix` text carries the open findings, so a custom `fix`
  must tell the implementer to call `list_findings` and close each one
  with `update_finding`, or keep `fix = "/fix-findings"`.

`[[loop]] prompt` (mission) is standing instructions for the **conductor
session** when the human talks — charter, whether to open a PR, whether
to reopen scope. The engine walks the graph; this is not a free-form
“do whatever satisfies this prompt,” and it is not sent to specialists.
Omit it if the conductor role prompt plus the live task are enough.
Role prompt: who they are. Phase prompt: that turn’s user text after
ExpandComposer. Composer `[[prompt]]` with `loop =` is the first user
bubble/wire (often a short display label), not this field.

## Prompts

Keep them as short as the shipped Loop pack. Reviewer / editor prompts
must calibrate severity (the engine does not reclassify):

- critical / high — ship-blockers
- medium — should-fix before done / PR
- low / info — nits

## After writing

```
recur bundles lint ~/.recur/bundles/<Name>
```

A bad pack is skipped at discover (Settings can show why). Reload Recur
or revisit Settings → Bundles; the loop picker is name-sorted from the
catalog. Main working copy still cannot engage Loop.
