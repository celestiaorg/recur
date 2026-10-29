# Cursor provider

Recur can run loops with Cursor's agent instead of Claude Code. This documents
how it works, why it is built this way, and what it cannot do.

## Architecture

Cursor exposes three client surfaces — the `cursor-agent` CLI, a TypeScript SDK,
and a Python SDK. They are **all the same client**: each speaks Cursor's private
`agent.v1.AgentService` ConnectRPC API at `api2.cursor.sh`. The CLI and the
TypeScript SDK contain byte-identical service definitions; the Python SDK does
not reimplement anything, it vendors a Node runtime plus the compiled TypeScript
SDK and drives it over a local server called the **bridge**.

Recur uses that bridge — **one process per worktree**, pooled by the engine.
A single shared bridge inherits the macOS GUI app's cwd (`/`), and Cursor's
shell tool falls back to `process.cwd()` when the model omits
`working_directory`, so agents would `pwd` at disk root even with
`local.cwd` set. Recur starts each bridge with `--workspace <worktree>` and
`cmd.Dir = <worktree>`, multiplexing agents for that worktree by `agentId`.

```
Recur  ──Connect/JSON──▶  cursor-sdk-bridge (Node, cwd=worktree A)
       ──Connect/JSON──▶  cursor-sdk-bridge (Node, cwd=worktree B)
                                        │
                                        ▼  agent.v1 / protobuf
                                 api2.cursor.sh
```

The bridge is language-agnostic by design (its `--tool-callback-url` flag is
documented as "Python/host"), advertises `protocolVersion: "sdk.v1"` plus a
capability list, and speaks ~120 message types — versus 907+ for `agent.v1`.

Key point: **Cursor's backend owns the agent loop.** The client is a
tool-execution host. The server sends tool-call requests down the stream and the
client executes them locally. This is why several things below are impossible
rather than merely unimplemented.

### Why the bridge and not the CLI

The `cursor-agent` CLI reads MCP configuration only from
`<projectRoot>/.cursor/mcp.json` and `~/.cursor/mcp.json` — both paths are
hardcoded to `homedir()`/`projectRoot` in its bundle, and no flag or environment
variable relocates them (`CURSOR_CONFIG_DIR` resolves the config root but MCP
discovery bypasses that helper). Using the CLI would mean writing a config file
into each loop's worktree, which dirties the tree the diff/PR flow reads and can
clobber a user's own file.

The bridge takes `mcpServers` **inline, per agent**. Nothing is written to disk.

## Code map

The provider is `providers/cursor/src` (`recur-provider-cursor`).

| File | Role |
|---|---|
| `bridge.rs` | Spawn the bridge, parse its stderr discovery line, process-group lifecycle |
| `connect.rs` | Connect protocol: JSON unary + 5-byte-framed server streams |
| `client.rs` | Typed `sdk.v1` calls (CreateAgent, ResumeAgent, Send, …) |
| `events.rs` | Run-stream event decoding |
| `locate.rs` | Bridge resolution |
| `install.rs`, `attribution.rs` | On-demand download + unpack of the pinned bridge, and its attribution patch |
| `runner.rs`, `turn.rs`, `steer.rs` | The runner: `AgentSpec` ⇄ Cursor, events → `ParsedEvent` |
| `tools.rs` | Cursor's tool vocabulary → the common one |
| `pool.rs`, `provider.rs` | Bridge per worktree, lazy start, the `Provider` impl |
| `status.rs` | Settings probes (status, Connect) |
| `crates/agent/src/display` | The display boundary: raw provider data → UI shapes |
| `crates/services/src/app/model_catalog.rs` | Grouped model catalogue across configured providers |
| `crates/usage/src/cursor` | Sidebar plan-quota fetch (dashboard API; not the API key) |
| `crates/domain/src/model.rs` | `ModelRef`: the `provider/id` selection type |

## Selecting a provider

There is no per-role provider setting. Each role's model is a fully-qualified
`ModelRef` — `claude-code/opus`, `cursor/composer-2.5` — and the prefix is what
routes the turn. Picking a model IS picking the agent that runs it. Recur's
picker lists every enabled, configured provider's models in one grouped list.

Settings → Providers can turn a provider **off** without clearing credentials.
A turned-off provider is omitted from pickers and the engine will not start it.

Two consequences:

- **Roles can run on different providers.** An orchestrator on Opus with a coder
  on Composer is just two model picks; `Engine.runAgent` resolves the ref, picks
  the runner, and hands the runner the bare id.
- **There is one source of truth.** A role's model and the agent that runs it
  cannot disagree, because they are the same field.

Claude Code is offered unless turned off in Settings → Providers. Cursor
appears in the pickers once Cursor is on (`ProviderEnabled`) and credentials
are present (`Settings.CursorConfigured`) — Connect mints the SDK key.
A role
pointed at Cursor without a working key, or at a turned-off provider, fails
the turn with a clear message rather than silently falling back to Claude
Code, which would run and bill a model the user did not choose.

## Every runner must be mirrored

`Engine.runnerFor` applies `withMirroring` at **one** place, to whatever
`resolveRunner` returns. This is deliberate, and load-bearing.

The first Cursor build wrapped the Claude runner in `New()` and the injected
test runner in `SetRunner`, but returned the Cursor runner raw. Cursor turns
therefore bypassed the session mirror entirely: the run succeeded and edited
files, but wrote no session, no transcript, no `tool_use` blocks and no
`session.delta` events — so the Activity panel showed neither streaming nor tool
calls, and the database held nothing for the turn. Nothing errored; there was
simply no record.

Two invariants keep it fixed, both with tests that fail if the fix is reverted:

- `runnerFor` wraps; `resolveRunner` does not. A new provider cannot forget the
  wrap, because branches never wrap.
- `AgentSpec.Provider` carries the resolved provider. `runAgent` replaces the
  qualified ref with the **bare** model id before the runner (and therefore the
  mirror) sees the spec, so re-deriving the provider from `spec.Model` inside the
  mirror always yields `claude-code` and silently mislabels every Cursor session.

## Tool rendering: raw storage, common display

Two rules that pull in opposite directions:

1. **The session store is the source of truth** and holds whatever the provider
   actually sent — Cursor's `shell` with `{command, timeout}` and its nested
   `{"status":…,"value":{…}}` result, verbatim. A transcript that silently
   rewrites provider data is no longer a record of what happened, and it makes a
   provider's own quirks undebuggable from the database.
2. **The UI shows one vocabulary across providers**, so a Cursor `shell` call and
   a Claude `Bash` call render in the same terminal card.

So translation happens at the **display boundary** (`recur_agent::display`),
never on the way in. `ParsedEvent` carries the provider slug and the provider's
raw tool name, args and result; `displayTool` / `displayToolResult` render them.

```
CursorRunner ──raw──▶ ParsedEvent{Provider, Tool:"shell", ToolInputRaw:{command,timeout}}
                          │
                          ├── mirror ──▶ message_content: name="shell", input={command,timeout}
                          │                                              (source of truth)
                          └── displayTool() ──▶ ChatItem{Tool:"Bash", Input:{command}}
                                                 session.step / feed rows
```

Because the mapping is applied at read time, changing it re-renders existing
transcripts rather than leaving old rows stale.

| Cursor (stored) | args (stored) | Displayed | display input |
|---|---|---|---|
| `shell` | `{command, timeout}` | `Bash` | `{command}` |
| `read` | `{path}` | `Read` | `{file_path}` |
| `edit` | `{path}` | `Edit` | `{file_path}` |
| `delete` | `{path}` | `Delete` | `{file_path}` |
| `glob` | `{globPattern, targetDirectory}` | `Glob` | `{pattern, path}` |
| `grep` | `{pattern, path, offset}` | `Grep` | `{pattern, path}` |
| `updateTodos` | `{todos[]}` | `TodoWrite` | `{todos}` (statuses normalized) |
| `task` | `{description, prompt, …}` | `Task` | `{description, prompt, subagent_type?, model?}` |
| `createPlan` | `{plan}` (markdown; result is `{}`) | `CreatePlan` | `{plan}` |
| `mcp` | `{providerIdentifier, toolName, args}` | `mcp__<server>__<tool>` | the tool's own args |

Claude Code defined the common vocabulary, so its calls pass through untouched.
Unknown Cursor tool names also pass through — a tool added after this build
renders with its raw name and a generic icon rather than vanishing.

Three things are easy to get wrong here, and each cost a round of debugging:

- **Results are nested and per-tool.** `value` has a different shape per tool
  (`stdout` for shell, `content` for read, `files[]` for glob, `diffString` for
  edit, `workspaceResults` for grep). A result block also cannot say how to read
  itself, so the transcript remembers the originating call's provider and tool
  name to unpack it.
- **Cursor's `edit` args carry ONLY the path.** The change itself is a unified
  diff in the *result*, so the Edit card parses that (`parseUnifiedDiff` in
  `lib/codehl.ts`) into the same `DiffRow[]` it builds from Claude's
  `old_string`/`new_string`. One diff viewer, two providers.
- **A tool call is a RUN of frames, not one running + one terminal.** `read`
  sends `running, completed`; `edit` sends `running, running, completed`
  (progress is re-reported). Emitting a `KindStep` per frame minted a second
  tool_use block per edit, and since the id map keeps only the last, the result
  attached to the second — so the first rendered as a bare input card with no
  diff. The runner folds frames into exactly one step and one result per call id.
- **A failed tool call gets NO terminal frame at all**, despite the SDK's own
  type saying otherwise. `SDKToolUseMessage.status` is documented as
  `"running" | "completed" | "error"`, but on SDK 1.0.31 (1.0.38 maps tool
  status the same way) a read of a missing file emits `running` and then
  nothing. Cursor sends `running`
  and then simply stops: no result, no error status, and the run still finishes
  successfully. The failure is mentioned only in the model's own prose ("None of
  them exist in this directory") — never machine-readably. Left unresolved, the
  transcript stores a `tool_use` with no `tool_result`, and the card falls back
  to dumping the raw input, so a failed read renders as a bare `{"file_path": …}`
  card. The runner closes every still-open call when the run ends, with an error
  result stating only what is known: no result arrived.

  WHEN they are closed matters as much as that they are. It happens at the next
  reasoning or reply frame, not only at the end of the run: the model cannot
  think or speak again until its outstanding tool results are back, so by then an
  unreported call is definitively finished. Closing only at the end left a failed
  read showing "Reading go.mod…" for the rest of the session with later work
  rendered above it — while the very next thought was already "go.mod does not
  exist." A late real result is harmless; it overwrites the synthetic one by
  call id.
- **Any non-running status is terminal.** Matching only `completed`/`failed` let
  an undocumented status silently drop a result, leaving the call with no output
  and its card showing only the input. A terminal frame with no preceding
  `running` still emits its step, so a result never points at a tool_use block
  that was never written.
- **An assistant reply is a run of CHUNKS, not one complete message.** Both
  providers stream. The difference is that Claude Code sends `stream_event`
  text deltas for the live view AND a final `assistant` line carrying the
  COMPLETE block, which is what the mirror persists — whereas Cursor's
  `assistant` frames ARE the deltas, with no summary block at the end. Treating
  each as complete (correct for Claude) is what broke. Storing a block per frame split replies mid-sentence
  and mid-markdown — a fenced code block opened in one block and closed in the
  next, so a reloaded transcript rendered ```` ```bash\ngo ```` and
  ```` run .\n``` ```` as two broken fragments, and `Result.Text` held only the
  last fragment. The runner accumulates and writes ONE block per contiguous run
  of text, while still emitting a `KindTextDelta` per chunk so the reply streams
  live (the mirror's `gotDelta` guard suppresses the duplicate publish).
- **Nothing about an edit can be streamed.** Its `running` frames carry only
  `{"path": …}` (18 bytes, twice), and the diff arrives whole with `completed`.
  The SDK's `enableDeltas` flag is ON so Recur's `onDelta` sees
  `InteractionUpdate` as documented: `turn-ended` (context meter) and
  `summary` / `summary-started` / `summary-completed` (compact cards).
  Stock cursor-sdk 1.0.38's `RunInteractionAccumulator.apply` still skips
  those summary members of the union; Recur's install minify patch
  restores the documented callback. `SDKMessage` `type: "task"` is
  task/subagent status (`SDKTaskMessage`), not compaction — even though
  the SDK also maps summary text onto that stream as a side channel. Token-level `token-delta` / `partial-tool-call`
  frames are ignored — an edit's arguments are still just the path, so those
  extras do not help the card. A call in flight therefore renders a stated
  pending state rather than a dump of its input JSON.
- **Reasoning is a run of frames, not one event.** Cursor streams thinking as
  text chunks terminated by an empty frame carrying `thinking_duration_ms`. The
  runner accumulates and emits a single `KindThinking`, flushing on the next
  tool call, assistant reply or run result — otherwise reasoning is lost and the
  Activity panel shows an unexplained gap before each tool call. Live reasoning
  streams as `KindThinkingDelta` (gated on `IncludePartials`, like Claude's
  `--include-partial-messages`); emitting it as a text delta would splice the
  model's private reasoning into its visible reply.

Text and reasoning close each other, and both flush at a tool call, at the run
result, and at the end of the turn — so transcript order matches what happened,
and a stream that dies mid-reply still records what arrived.

`SDKToolUseMessage.truncated` flags args or results the SDK clipped for size.
That is stored on the tool_result block (provider fact) and shown as a
"truncated" badge, so a clipped file read or command output is not presented as
the whole thing.

Todo statuses are `pending | inProgress | completed`; only the camelCase one
differs from the common vocabulary (Claude Code's TodoWrite uses `in_progress`),
so it is normalized and everything else passes through. It renders as a checklist
with per-item state, and the collapsed row shows `done/total` plus the item
currently in flight rather than a truncated JSON array. It is labelled "todos",
not "plan" — this is a flat checklist the agent keeps for itself, distinct from
Recur's own planning concepts (the Maestro's plan, plan mode).

`createPlan` is Cursor plan mode's native plan surface (args.plan, result `{}`).
Recur prefers its own MCP `submit_plan` / `update_plan_step` for structured
plans; prompts nudge Cursor that way, but createPlan cannot be disabled via the
SDK. `emitStepOnce` defers createPlan running frames and emits on terminal with
the richest args seen. If a planning turn or Plan-mode chat only used createPlan
(no `submit_plan`), `adoptProsePlan` files the full markdown in `Plan.Body`
(with a first-paragraph headline in `Summary`) and mirrors it into
DefinitionOfDone — enough for the Plan panel and approval gate, without
inventing fake todos.

The shapes above are transcribed from live bridge traffic. Cursor documents
neither the tool list nor the argument schemas, so `cursortools_test.go` pins
verbatim captured payloads as fixtures.

## Limitations

These are properties of Cursor's backend, not gaps in this client. The Settings
pane states them to the user.

**No custom system prompt.** Cursor exposes none. The CLI has a hidden
`--system-prompt` flag, but it is gated to Anysphere/OpenAI accounts and the
backend rejects it (`[invalid_argument] unknown option '--system-prompt'`).
Recur prepends the role prompt to the **first** user Send of an agent
inside `<role-instructions>` tags. Follow-up Sends on a resumed agent omit
the fence (the history already has it). A CreateAgent fallback after a
failed resume still includes it.

**Mid-turn steering (D49) prefers `run.steer`.** Cursor's SDK can append a
user message to a run in flight (`SteerRun` on Recur's patched bridge).
`complete_delivered` means the turn has the text — Recur does not CancelRun or
resend. `revert_to_followup`, a missing RPC, or any other failure maps to
`CancelRun` then `Send` of the concatenated steers on the **same** agentId —
still inside one `Run`, so the engine does not see a failed specialist turn.
A user Stop still uses `ctx` cancel and must `CancelRun` the follow-up run
(steer cancel does not consume the Stop `Once`). Newly spawned specialists
still get `pendingSteer` in their initial Prompt. Workspace pair-chat
follow-ups use the same Inject channel (`InjectPlain`: no Maestro prefix) so a
message sent while any provider is running injects immediately instead of
sitting queued until the turn ends. Claude writes the follow-up as a
stream-json stdin user message; Codex calls `turn/steer`.

**No per-role tool allowlist.** Cursor has no equivalent of `--allowedTools`.
Read-only roles are mapped onto Cursor's `plan` mode; other roles get the full
tool set, scoped only by the loop's isolated worktree.

**No dollar cost.** `GetUsage` is cloud-only and errors for local agents. Runs
report token counts for the context meter. Recur does not convert tokens to USD.

**Plan usage is unofficial.** Recur's `crsr_` API key cannot read the Included
Usage dashboard. The sidebar footer reads account quotas from Cursor's
undocumented `api2.cursor.sh` DashboardService (`GetCurrentPeriodUsage`,
`GetAggregatedUsageEvents`). Auth is Recur Connect only: session tokens at
`<dataDir>/cursor-usage-auth.json` (0600). Recur does not read local
`state.vscdb` or the `cursor-agent` keychain. Connect uses the same unofficial
PKCE deep-link as `cursor-agent` (`loginDeepControl` + `auth/poll`), then mints
a 90-day user API key named `Recur` via `DashboardService/CreateUserApiKey`
(official `Cursor.auth.login()` does this and drops the session). Recur keeps
the session for plan usage — a `crsr_` key cannot read the dashboard — and
writes the minted key into Settings so the SDK can run agents. Disconnect
clears that key; a leftover paste is dropped when there is no Connect file.
Recur remints
within 7 days of expiry and revokes the old key
(`ListUserApiKeys` + `RevokeUserApiKey`). The flow can break without notice. Session JWTs last ~60 days; Recur
refreshes them within 7 days of `exp`.

**Model ids differ.** Cursor publishes no aliases (`ListModels` returns
fully-qualified ids with empty alias lists), so `sonnet`/`opus` are not valid
there. Cursor's ids are fetched live into the model catalogue rather than
hardcoded anywhere.

## Installing the bridge

Recur does not vendor the bridge — it is ~50MB compressed and ~155MB unpacked
per platform, almost all of it a bundled Node runtime. **Settings → Providers →
SDK → Install** fetches it on demand (`cursor.Install`):

1. Fetch the **pinned** release `https://pypi.org/pypi/cursor-sdk/<PINNED_SDK_VERSION>/json`
   (not "latest") and pick the wheel for this OS/arch. The pin matches the
   minify anchors used by the attribution patch — bump both together. Filenames
   are MATCHED rather than a PEP 425 tag being constructed — macOS wheels are
   `macosx_11_0_*` and Linux ones carry a compound
   `manylinux2014_x86_64.manylinux_2_17_x86_64`.
2. Download to a staging dir, verifying PyPI's published sha256.
3. Extract only `cursor_sdk/_vendor/bridge/**`, preserving the executable bit
   (without it `bin/node` and the launcher come out non-executable) and
   rejecting any member that would escape the destination.
4. Apply Recur's attributionConfigProvider + getAgentSkills stub; install fails if anchors miss.
5. Rename into `<dataDir>/cursor-bridge` only once complete, so an interrupted
   download cannot leave a half-extracted bridge for `Locate` to hand the engine.
6. Record the launcher path in `Settings.CursorBridgePath` (display/history only).

Progress is published on the bus as `cursor.install` events (phase, message,
percent).

`cursor.Locate` only returns `<dataDir>/cursor-bridge`. `$RECUR_CURSOR_BRIDGE`,
`Settings.CursorBridgePath`, and pip wheels are not production resolution paths
— those copies skip the skill stub. Agent runs fail if that tree is missing or
if `EnsureAttributionPatch` cannot match minify anchors (reinstall from Settings).

`$RECUR_CURSOR_BRIDGE` remains the **smoke-test** seam (`ResolveLauncher`); it
does not affect the running app. For a live SDK test:

```sh
pip download cursor-sdk --no-deps -d /tmp/cs
unzip -q /tmp/cs/*.whl -d /tmp/cs/w
export RECUR_CURSOR_BRIDGE=/tmp/cs/w/cursor_sdk/_vendor/bridge
```

## Re-pinning cursor-sdk

Install never follows PyPI "latest": `PINNED_SDK_VERSION` (`install.rs`) and
the attribution anchors (`attribution.rs`) must move together. To detect drift:

```sh
mise run check:cursor-sdk-pin
# or: RECUR_CURSOR_SDK_PIN_CHECK=1 cargo test -p recur-provider-cursor --lib check_cursor_sdk_pin_drift
```

A scheduled workflow (`.github/workflows/cursor-sdk-pin.yml`) runs the same
check weekly. When it fails, agents should use the project skill
`.agents/skills/repin-cursor-sdk/SKILL.md`: download the new wheel, refresh
cjs/esm minify anchors, bump the pin + the `pinned_sdk_version_in_pypi_url`
hardcode, then validate with
`RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 cargo test -p recur-provider-cursor --lib attribution_patch_against_pinned_wheel`.
The same pass also rewrites `RunInteractionAccumulator.apply` so summary
`onDelta` events are forwarded (`sdk_summary_delta_anchors`); if that minify
site moves, refresh those needles too.

## Tests

Hermetic tests (run in CI, no bridge needed): `cargo test -p recur-provider-cursor`
covers event decoding against frames captured verbatim from a live bridge
(`events`; these pin the wire quirks: payload nested under `message`,
snake_case payload keys with camelCase nested structs, int64s as JSON strings
in the result but numbers in usage messages, `running` (not `started`) as the
in-flight tool status, and every MCP tool reporting `name: "mcp"`), spec mapping
and event translation (`runner`, `turn`), provider routing and its failure
modes (`provider`), and installs (`install`: platform matching against real
published filenames, executable bits surviving extraction, zip-slip rejection).
The catalogue shape is `crates/services/src/app/tests/model_catalog.rs`.

A real download+install+start (~50MB from PyPI):

```sh
RECUR_CURSOR_INSTALL_TEST=1 cargo test -p recur-provider-cursor --lib install_real_smoke -- --nocapture
```

Live tests (skipped unless the env vars are set; they cost real Cursor usage):

```sh
RECUR_CURSOR_BRIDGE=... CURSOR_API_KEY=... cargo test -p recur-provider-cursor smoke -- --nocapture
```

The live tests assert on the **MCP server's own wire log**, not just the model's
answer. That matters: an early version appeared to pass because the model had
found a config file on disk and shelled out to `curl` rather than using MCP at
all. Asserting that `initialize`/`tools/list` actually arrived is what catches
this.
