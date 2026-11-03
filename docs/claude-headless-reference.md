# Claude Code Headless Mode: Verified Reference (2026)

> Captured from `claude --help` (installed **v2.1.126**) cross-referenced with
> `code.claude.com/docs/en/headless`, `/cli-reference`, `/agent-sdk/*`, `/mcp`,
> `/authentication`. This is the ground truth for Recur's agent runner (PRD §8)
> and the Recur MCP server (PRD §8.3). Re-verify before shipping M2/M3.

Two meta-facts:
1. `claude --help` is **not exhaustive**: a flag's absence from `--help` does
   not mean it is unavailable (e.g. `--max-turns`, `--permission-prompt-tool`,
   `--system-prompt-file` are documented but not in the v2.1.126 help).
2. `--permission-mode` choices in v2.1.126: `acceptEdits, auto, bypassPermissions,
   default, dontAsk, plan`: **no `acceptAll`**.

## 1. Flags Recur relies on

| Flag | Behavior |
|---|---|
| `-p`, `--print` | Non-interactive. Reads stdin. Trust dialog skipped. |
| `--output-format <text\|json\|stream-json>` | `stream-json` = NDJSON realtime. Only with `--print`. |
| `--input-format <text\|stream-json>` | `stream-json` = multi-turn streaming input over stdin. |
| `--verbose` | **Mandatory with `--output-format stream-json`.** Not positional. |
| `--system-prompt <p>` | Replaces entire default system prompt (role definition). |
| `--append-system-prompt <p>` | Appends to default prompt. |
| `--allowedTools <tools...>` | Comma/space list; permission-rule syntax e.g. `"Bash(git diff *)" "Read" "mcp__recur__*"`. Auto-approves. |
| `--disallowedTools <tools...>` | Deny rules. Bare name removes tool from context (`"Edit"`, `"mcp__*"`). |
| `--mcp-config <configs...>` | Load MCP servers from JSON **files or inline strings**. |
| `--strict-mcp-config` | Only servers from `--mcp-config`; ignore ambient `.mcp.json`/user/plugins. |
| `--permission-prompt-tool <mcp_tool>` | MCP tool that answers permission prompts non-interactively. **Verified against v2.1.198** (see §1a). Absent from `--help`. |
| `--permission-mode <mode>` | `default\|acceptEdits\|plan\|auto\|dontAsk\|bypassPermissions`. `acceptEdits` does NOT auto-approve MCP/Bash. |
| `--resume, -r [id]` | Resume by session id/name. Scoped to project dir + worktrees. |
| `--continue, -c` | Continue most recent conversation in cwd. |
| `--model <m>` | Alias (`opus`/`sonnet`/`haiku`/`fable`) or full id (`claude-sonnet-4-6`). |
| `--add-dir <dirs...>` | Extra read/edit dirs (file access, not config discovery). |
| `--settings <file-or-json>` | Settings file or inline JSON; can carry `apiKeyHelper`. |
| `--max-budget-usd <amt>` | Hard USD spend cap (print mode). |
| `--max-turns <n>` | Limit agentic turns (documented; not in v2.1.126 help). |
| `--session-id <uuid>` | Use a specific session id. |
| `--fork-session` | On resume, branch a new session id. |
| `--json-schema <schema>` | Structured output → `structured_output` field (with `--output-format json`). |
| `--bare` | Minimal mode (see §2). |
| `--disable-slash-commands` | Disable all native skills / slash expansion. Recur passes this so only `mcp__recur__list_skills` / `activate_skill` load SKILL.md. Independent of `--bare` (D8). |
| `--disallowedTools Skill` | Bare name removes Claude's Skill tool from context (same pattern as `AskUserQuestion`). |

Flags are **not order-sensitive** relative to each other; only dependency rules
(e.g. `--verbose` required by `stream-json`). Prefer piping large input via stdin.

## 1a. `--permission-prompt-tool` contract (verified v2.1.198)

The flag is undocumented in `--help`, so this was captured by pointing a real
`claude -p` run at a probe MCP server and logging both directions.

**Request.** The CLI calls the named MCP tool itself (a plain `tools/call`) with:

```json
{"tool_name": "Write",
 "input": {"file_path": "/tmp/x.txt", "content": "hi"},
 "tool_use_id": "toolu_012Jw…"}
```

`_meta` also carries `claudecode/toolUseId` and a `progressToken`.

**Response.** The tool result's text must be a JSON *document*, not prose:

```json
{"behavior": "allow", "updatedInput": {…}}
{"behavior": "deny",  "message": "why not"}
```

`updatedInput` is **required** on allow (it is what the tool actually runs with;
echo the input back to leave it unchanged). A deny is a *successful* tool result:
the tool does not run and the refusal appears in the final result's
`permission_denials[]`.

Verified properties Recur depends on:

- **The prompt tool is exempt from `--allowedTools`.** The probe run passed no
  `--allowedTools` at all and the tool was still invoked.
- **The prompt tool MUST be advertised by the MCP server's `tools/list`.** The
  CLI resolves the flag's tool name against the advertised inventory at startup
  and exits with `Error: MCP tool mcp__recur__permission_request (passed via
  --permission-prompt-tool) not found` if it is missing (verified against the
  installed CLI on 2026-08-07 via `TestRealPermissionPromptE2E`). Exemption from
  `--allowedTools` does **not** mean the tool can be hidden from `tools/list`
  while staying callable: do not re-attempt an advertise-vs-callable split for
  Claude bindings.
- **The CLI blocks for as long as the tool takes.** An 8s hold in the handler
  produced a 12s run that then proceeded normally: this is what lets a human
  actually decide.
- **It only fires under a mode that asks.** With `--permission-mode default`,
  allowlisted tools are auto-approved without consulting it, and some Bash
  commands the CLI classifies as safe (e.g. `echo`) never reach it either. Under
  `bypassPermissions` or `dontAsk` it is never called.

## 2. `--bare` (real)

Skips hooks, LSP, plugin sync, attribution, auto-memory, keychain reads, and
CLAUDE.md auto-discovery. Sets `CLAUDE_CODE_SIMPLE=1`. **Anthropic auth is strictly
`ANTHROPIC_API_KEY` or `apiKeyHelper` (via `--settings`): OAuth, keychain, and
`CLAUDE_CODE_OAUTH_TOKEN` are NOT read in bare mode.** Has Bash/read/edit tools.
"Recommended for scripted/SDK calls; will become the default for `-p`."

**Recur impact:** if we use `--bare` we MUST provide an explicit credential. See §6.

## 3. stream-json OUTPUT: NDJSON line schemas

Invoke: `claude -p "..." --output-format stream-json --verbose [--include-partial-messages]`.
Each line is one JSON object. Switch on `.type`; **ignore unknown types** (~30 in the union).

- **`system`/`init`** (first line): `{ type:"system", subtype:"init", session_id, uuid, model, tools[], mcp_servers[]{name,status}, cwd, permissionMode, apiKeySource, slash_commands[], output_style, plugins[], plugin_errors[] }`. Read `mcp_servers[].status` (`connected|failed|pending`) to confirm the Recur MCP attached.
- **`assistant`**: `{ type:"assistant", uuid, session_id, message:<Anthropic message>, parent_tool_use_id }`. `message.content[]` carries `tool_use` blocks: `{type:"tool_use", id:"toolu_...", name, input}`. MCP calls have `name` starting `mcp__`. Lines with `parent_tool_use_id` are Task-subagent events multiplexed onto the parent stream: Recur keeps nested `tool_use` / `tool_result` / text / thinking (rendered inside the parent Task card) and drops nested `result` / `init` so they cannot close or speak as the parent turn. Nested `stream_event` text/thinking deltas stream live inside the card.
- **`user`**: `{ type:"user", message:<param>, parent_tool_use_id, tool_use_result? }`. `tool_result` blocks live here: `{type:"tool_result", tool_use_id, content[], is_error}`. Same `parent_tool_use_id` nesting as assistant; background-Task launch acks (`Async agent launched successfully` / agentId plumbing) are not surfaced.
- **`result`** (LAST line). Success: `{ type:"result", subtype:"success", session_id, duration_ms, duration_api_ms, is_error, num_turns, result:<final text>, stop_reason, total_cost_usd, usage, modelUsage, permission_denials[], structured_output? }`. Error: `subtype:"error_max_turns"|"error_during_execution"|"error_max_budget_usd"|"error_max_structured_output_retries"`: **no `result` field**. Branch on `subtype` before reading `result`.
- Also: `stream_event` (partial deltas), `system`/`api_retry` (`attempt,max_retries,retry_delay_ms`), `system`/`plugin_install`.

Capture session id for resume via `--output-format json | jq -r '.session_id'` (single object), or the final `result` line of a stream.

## 4. stream-json INPUT: multi-turn over stdin

With `--input-format stream-json`, write newline-delimited objects to stdin:
```json
{"type":"user","message":{"role":"user","content":"..."},"parent_tool_use_id":null}
```
`content` may be a string or a content-block array (text/image). `parent_tool_use_id`
is `null` for top-level turns. The agent stays alive and processes queued turns
(used for Maestro chat). For stateless one-shot continuation, use `--resume <id>` instead.

## 5. MCP config (`--mcp-config`) + tool naming

Top-level key `"mcpServers"`. Variants:
- **stdio** (default): `{ "command":"...", "args":[...], "env":{...} }`
- **http**: `{ "type":"http", "url":"https://.../mcp", "headers":{...} }`
- **sse**: `{ "type":"sse", "url":"...", "headers":{...} }`
- **sdk** (`type:"sdk"`, in-process): **only from the TS/Python SDK, NOT via `--mcp-config`.**

`${VAR}` / `${VAR:-default}` expansion works in command/args/env/url/headers.

**MCP tools name as `mcp__<server>__<tool>`** (e.g. `mcp__recur__report_finding`).
Wildcard `mcp__recur__*` allows the whole server. `acceptEdits` does NOT auto-approve
MCP tools: use explicit `allowedTools` wildcards.

**Recur decision:** the engine hosts the Recur MCP as an **HTTP server on
localhost** (the `claude` CLI is a separate subprocess, so in-process `sdk` type is
unavailable). Pass `--mcp-config '{"mcpServers":{"recur":{"type":"http","url":"http://127.0.0.1:<port>/mcp"}}}' --strict-mcp-config`.
Per-agent tokens in headers can scope which loop/agent a call belongs to.

## 6. Auth (precedence, highest first)

1. Cloud provider when `CLAUDE_CODE_USE_BEDROCK|USE_VERTEX|USE_FOUNDRY` set.
2. `ANTHROPIC_AUTH_TOKEN` (Bearer; for gateways).
3. `ANTHROPIC_API_KEY` (X-Api-Key): **always used under `-p` when present**.
4. `apiKeyHelper` (settings script; refreshes on 401 / 5 min).
5. `CLAUDE_CODE_OAUTH_TOKEN` (long-lived; `claude setup-token`; not read under `--bare`).
6. Subscription OAuth from `/login` (default; not read under `--bare`).

**Recur decision (provisional, §21 #7):** v1 supports the user's existing Claude
Code login (do NOT use `--bare`, so subscription OAuth/keychain works) as the default
path, and an optional `ANTHROPIC_API_KEY` override. Defer Bedrock/Vertex. Recur never
logs credentials.

## 7. Models

Aliases: `opus`, `sonnet`, `haiku`, `fable`. Full ids like `claude-sonnet-4-6`.
Recur maps roles → models in settings (PRD §18): opus for Maestro/Reviewer judgment,
sonnet for Coder/Tester/QA. Prefer aliases to track latest snapshots.

## 8. Minimal known-good orchestration command
```bash
claude -p \
  --output-format stream-json --input-format stream-json --verbose \
  --model sonnet \
  --mcp-config '{"mcpServers":{"recur":{"type":"http","url":"http://127.0.0.1:PORT/mcp"}}}' \
  --strict-mcp-config \
  --allowedTools "Read,Edit,Bash,mcp__recur__*" \
  --permission-mode default --permission-prompt-tool mcp__recur__permission_request \
  --append-system-prompt "<ROLE PROMPT>"
# Write {"type":"user",...} lines to stdin; read NDJSON; final {"type":"result",...} carries session_id/total_cost_usd/is_error/subtype.
# Read-only roles: narrower --allowedTools + --disallowedTools Edit,Write,NotebookEdit.
# The permission mode is global (Settings.BypassPermissions, default off: shown above).
# With it ON, swap those two flags for: --permission-mode bypassPermissions
```

## Spec deltas (vs PRD §8 as written)
- `--bare` is real but **strips OAuth**: only use with an explicit API key/helper. PRD's bare example would break subscription-login users; v1 default avoids `--bare`.
- No `acceptAll` permission mode. The mode is machine-global (Settings.BypassPermissions, default OFF → `default` + `--permission-prompt-tool` everywhere, see §1a; on → `bypassPermissions` everywhere). Read-only turns that omit Edit/Write also pass `--disallowedTools`.
- `--verbose` is a hard requirement with stream-json (PRD notes this).
- App-hosted MCP must be **http/sse/stdio**, not in-process `sdk`, because the CLI runs as a child process. → HTTP server on localhost.
