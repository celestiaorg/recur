# Codex provider

For browser sign-in **without installing Codex**, use the separate
[direct ChatGPT provider](chatgpt-provider.md) in Settings → Providers → OpenAI → ChatGPT account.

Recur can run loops with OpenAI Codex via **`codex app-server`** (direct
JSON-RPC over stdio). This is option **D** from the Codex integration
comparison: a native client, not the process-per-turn `codex exec` CLI and not a
Python/TS SDK wrapper.

## Architecture

```
Recur ──JSON-RPC stdio──▶ codex app-server
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

All in `providers/codex/src` (`recur-provider-codex`) unless noted.

| File | Role |
|---|---|
| `locate.rs` | Binary resolution (`$RECUR_CODEX` → Settings → PATH) |
| `server.rs` | Spawn app-server, handshake, process-group kill |
| `rpc.rs` | JSON-RPC framing (header omitted on the wire) |
| `client.rs` | Typed `thread/*` / `turn/*` / `model/list` / `account/read` |
| `mcp.rs` | `--config mcp_servers…` argv builder |
| `runner.rs`, `items.rs`, `approvals.rs` | The runner: `AgentSpec` ⇄ app-server, items, server requests |
| `status.rs`, `login.rs` | Settings probes and managed browser sign-in |
| `catalog.rs` | Live `model/list` → picker group |

## Selecting Codex

There is no per-role provider setting. Role models are `ModelRef`s
(`codex/gpt-5.6-luna`). Codex appears in pickers only when it is on in
Settings → Providers **and** configured:

- `Settings.CodexAPIKey` non-empty, **or**
- `Settings.CodexChatGPTAuth` true (browser sign-in or reuse `codex login`). On by default for a
  fresh install when `~/.codex/auth.json` exists (a prior `codex login`).

Turning Codex off keeps the key / ChatGPT-auth flag; it just leaves the
pickers and the engine will not start it.

### Browser sign-in

In **Settings → Providers → OpenAI → Codex runtime**, choose **Sign in with ChatGPT**. Recur opens
the system browser and waits for sign-in. On success it selects ChatGPT auth,
clears any saved API key, and refreshes the account and model catalogue.
**Switch ChatGPT account** starts the same flow for another account.

This uses the official app-server `account/login/start` method with
`type: "chatgpt"`. Codex owns the localhost callback, credential persistence, and
token refresh. Credentials are stored in `<dataDir>/codex-home/auth.json`; Recur
does not write `~/.codex/auth.json`. A prior terminal login is not required.
An existing CLI login can still be imported by the **Use ChatGPT login** toggle
until the first browser sign-in. After that, a newer `~/.codex/auth.json` no
longer replaces Recur's credentials.

The login process stays alive for up to five minutes. **Cancel sign-in**, leaving
the Providers pane, and app shutdown cancel the attempt and reap its process.
Failed or cancelled attempts do not change the saved authentication settings.
The Codex binary is still required.

If sign-in cannot start, the provider card shows the error instead of disabling
the sign-in button. After installing Codex or saving a native **Binary** path,
discovery refreshes in place. Confirm the Binary row says **Found** and retry
**Sign in with ChatGPT**.

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
| Thinking | `--config model_reasoning_summary=auto` + `model_supports_reasoning_summaries=true`; stream `item/reasoning/summaryTextDelta` → `AgentEvent::ThinkingDelta`, completed `reasoning.summary` → `AgentEvent::Thinking` |
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

Hermetic: `cargo test -p recur-provider-codex` (RPC, MCP, locate, runner against a fake app-server; no binary needed).

Live smoke (optional):

```sh
RECUR_CODEX_E2E=1 RECUR_CODEX=/path/to/native/codex \
  cargo test -p recur-provider-codex --test suite -- codex_e2e_smoke
```

Automated checks: `mise run check`.
