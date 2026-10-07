# Codex provider

Recur can run loops with OpenAI Codex via **`codex app-server`** (direct
JSON-RPC over stdio). This is option **D** from the Codex integration
comparison — a Go client, not the process-per-turn `codex exec` CLI and not a
Python/TS SDK wrapper.

## Architecture

```
Recur (Go) ──JSON-RPC stdio──▶ codex app-server
                │                 cwd = worktree
                │                 CODEX_HOME = <dataDir>/codex-home
                │                 -c mcp_servers.recur.url=…
                ▼
         CodexRunner → ParsedEvent / Result → withMirroring
```

Why app-server:

- Mid-turn steer (`turn/steer`) maps to Recur’s D49 `AgentSpec.Inject`
- Server→client approvals / MCP elicitations are answerable in-process
- Thread resume (`thread/resume`) maps to `ProviderSessionRef`
- Recur MCP is injected via `--config` without writing into the worktree

Isolation (Cursor’s hard lesson):

- **Never** write `.codex/` or MCP config into the loop worktree
- One Recur-owned `CODEX_HOME` at `<dataDir>/codex-home` for the whole app
  (auth, sessions, trust). Worktrees are isolated via `cwd` on each thread/turn.
- ChatGPT auth may be *seeded* from `~/.codex/auth.json` into that home

## Code map

| File | Role |
|---|---|
| `internal/codex/locate.go` | Binary resolution (`$RECUR_CODEX` → Settings → PATH) |
| `internal/codex/server.go` | Spawn app-server, handshake, process-group kill |
| `internal/codex/rpc.go` | JSON-RPC framing (header omitted on the wire) |
| `internal/codex/client.go` | Typed `thread/*` / `turn/*` / `model/list` / `account/read` |
| `internal/codex/mcp.go` | `--config mcp_servers…` argv builder |
| `internal/engine/codex.go` | `CodexRunner` — `AgentSpec` ⇄ app-server |
| `internal/engine/provider.go` | `ProviderCodex` routing through `runnerFor` |
| `internal/app/codex_service.go` | Settings probes |
| `internal/app/model_catalog.go` | Live `model/list` → picker group |

## Selecting Codex

There is no per-role provider setting. Role models are `ModelRef`s
(`codex/gpt-5.6-luna`). Codex appears in pickers only when it is on in
Settings → Providers **and** configured:

- `Settings.CodexAPIKey` non-empty, **or**
- `Settings.CodexChatGPTAuth` true (reuse `codex login`)

Turning Codex off keeps the key / ChatGPT-auth flag; it just leaves the
pickers and the engine will not start it.

Binary: install the Homebrew cask (`brew install --cask codex`), set
`Settings.CodexBinPath`, or `$RECUR_CODEX`. The broken npm JS shim that
spawns a missing vendor binary is rejected. Point at the **native** binary
inside a full package layout (siblings like `codex-code-mode-host` matter);
a lone copied `codex` file can start app-server but fail workspace edits.

On first use of a worktree path, Recur appends
`[projects."<abs-cwd>"] trust_level = "trusted"` under that shared
`CODEX_HOME` so sandbox writes work in loop temp dirs.

## Contract mapping

| AgentSpec | Codex |
|---|---|
| Worktree | `cwd` + process `cmd.Dir` |
| Model | bare id on thread/turn |
| Flags `effort` | `turn/start` `effort` |
| Flags `fast` | `turn/start` `serviceTier` (`true` → `priority` / Fast; `false` → omit) |
| Thinking | `--config model_reasoning_summary=auto` + `model_supports_reasoning_summaries=true`; stream `item/reasoning/summaryTextDelta` → `KindThinkingDelta`, completed `reasoning.summary` → `KindThinking` |
| `fileChange` add | `Write` with `{file_path, content}` (body lives in `changes[].diff`) |
| `fileChange` update | `Edit` with path args + hunk-only unified diff result (`@@` without `---`/`+++`) |
| SystemPrompt | `developerInstructions` on thread/start and thread/resume (not also prepended into the user text) |
| AppendSystemPrompt | user-text suffix on a fresh thread / remount |
| TurnAppendPrompt | user-text suffix every turn (chat surface / PR) |
| MCPURL | `--config mcp_servers.recur.url=…` (`required=true`) |
| Playwright | `--config mcp_servers.playwright.command=npx …` |
| ResumeSession | `thread/resume` |
| Inject | `turn/steer` |
| Cancel | `turn/interrupt` + process-group kill |
| Reviewer | `sandbox: read-only` + turn `sandboxPolicy.networkAccess` (`gh pr view` / `gh pr diff`) |
| Settings.BypassPermissions off | `approval_policy=on-request`; command / file / `request_permissions` approvals → Recur permission prompts |
| Settings.BypassPermissions on | `sandbox: danger-full-access` + `approval_policy=never` (write roles); Recur auto-accepts residual requests |
| Settings.CodexNetworkAccess | `sandbox_workspace_write.network_access=true` + turn `sandboxPolicy.networkAccess` on workspace-write. Ask/Plan/Review always get read-only + network. |

Approvals: MCP Recur tools stay `default_tools_approval_mode=approve`. The
Providers → Permissions bypass toggle is shared with Claude Code. When it is
off, Codex `request_permissions` and sandbox exceptions
(`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
`item/permissions/requestApproval`) pause the turn until the human answers in
Recur’s permission UI — the same cards as Claude Code’s `permission_request`
tool. Codex does not use that MCP tool. MCP elicitations still auto-accept so
Recur’s own tools do not double-prompt.

## Limitations

- Role prompts add via `developerInstructions` (a Responses API `role: developer`
  message in the thread instruction stack, before AGENTS.md). They do not
  replace Codex's default stack. Recur does not also prepend them onto every
  user turn (the loop plan stays in `developerInstructions` only). Claude uses
  `--append-system-prompt` the same way.
- Recur does not estimate dollar cost from tokens (context meter only).
- App-server is spawned **per turn**. Recur MCP tokens are stable per
  (workspace, agent) across sequential turns (`Unregister` 404s until the next
  `Register`); overlapping same-identity turns get a distinct extra token.
  Thread history still persists in Recur’s `CODEX_HOME`.
- Every runner still goes through `runnerFor` → `withMirroring`. Never return a
  raw `CodexRunner` from a resolve branch.

## Tests

Hermetic: `internal/codex` RPC/MCP/locate tests (no binary needed).

Live smoke (optional):

```sh
export RECUR_CODEX=/path/to/native/codex
export RECUR_CODEX_E2E=1
go test ./internal/engine -run CodexE2E -count=1
```
