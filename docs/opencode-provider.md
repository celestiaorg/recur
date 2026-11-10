# OpenCode harness

Recur can run a model from an OpenAI-compatible endpoint inside
[OpenCode](https://opencode.ai) instead of its built-in compatible harness.
The user picks a model, then picks a harness. The stored selection is
`opencode/<source>/<model>`, e.g. `opencode/openrouter/openai/gpt-5`.

## Model sources versus harnesses

`ModelRef` names the harness in its provider field. Its id is the model
source and model id:

| Selection | Harness | Source | Model |
| --- | --- | --- | --- |
| `openrouter/openai/gpt-5` | built-in compatible harness | `openrouter` | `openai/gpt-5` |
| `opencode/openrouter/openai/gpt-5` | `opencode` | `openrouter` | `openai/gpt-5` |

`recur_domain::{harness_of, base_model_ref, compose_harness_ref}` are the
only places that split or join this form. An empty harness keeps the stored
ref unchanged, so settings and sessions written before the harness feature
still decode and run exactly as before.

The catalog stays source-owned. `ModelGroup` carries the harness choices for
its models, and the picker composes the harness prefix only when the user
picks one. The engine still resolves one provider per turn and never spells a
provider id.

## Registration

`recur-provider-opencode` implements `recur_agent::Provider` and registers in
`AppServices` next to the other providers. It supplies no catalog models. Its
`models()` is empty because the source endpoint owns the list.

The provider is available when `locate` finds an executable. Resolution order
is `$RECUR_OPENCODE`, `Settings.opencode_bin_path`, `opencode` on `PATH`, then
common install locations.

## One turn

1. Split `req.model` into source slug and source model id.
2. Look up the source `CompatibleProvider` in Settings.
3. Build an OpenCode config:
   - `provider` (singular in OpenCode 1.x): a `recur-<source>` custom provider
     using `@ai-sdk/openai-compatible`, `options.baseURL` with `/v1`, the API
     key, and any extra headers.
   - `mcp.recur`: the turn's Recur MCP URL, so host tools run in Recur.
   - `agent.recur`: a primary agent whose prompt is the turn's system prompt.
   - `permission`: Recur's tool allowlist translated to allow/deny rules.
   - `small_model`: the same model, for OpenCode's own title and compaction
     calls.
4. Spawn `opencode run --format json --model recur-<source>/<model>
   --agent recur --auto --dir <worktree>` with the config in
   `OPENCODE_CONFIG_CONTENT`. OpenCode picks its project from `--dir` or
   `$PWD`, not the process cwd, so both point at the worktree.
5. Convert JSON lines into `AgentEvent`s:
   - `text` and `reasoning` parts become `Text`/`Thinking`.
   - `tool_use` parts become `Step` plus `ToolResult`.
   - `step_finish` tokens become `Usage`.
   - `error` becomes a `Notice` and the failure text.
   - Each line carries `sessionID`, which becomes the resume handle.
6. Process exit ends the turn. Cancellation kills the process and returns the
   partial result as a canceled turn.

Tool names are lowercase (`bash`, `read`, `edit`, `todowrite`, ...). The
provider maps them to Recur's cross-provider vocabulary in `display_tool`;
the stored transcript keeps OpenCode's raw names.

## Permissions

Recur passes `--auto` so OpenCode does not wait for prompts a headless run
cannot answer. Safety comes from the generated `permission` object instead:

- `tools = Some("")` denies every known tool.
- An `allowed_tools` list denies edit, bash, task, webfetch, and external
  directory access when Recur does not allow them.
- `question` is always denied; Recur's own `ask_user` MCP tool is the
  interactive path.

## Limitations

- Only compatible endpoints are offered as sources. Account-backed providers
  (Claude Code, Cursor, Codex, ChatGPT) keep their own runtime.
- OpenCode uses Chat Completions, so it cannot run an endpoint that only
  implements the Responses API.
- Images are not forwarded.
- Live steering and partial text deltas are not supported; OpenCode emits
  completed parts.
- User-defined MCP servers are not forwarded in this version. Only Recur's
  host-tool MCP server is attached.
