# ACP provider (Antigravity, OpenCode)

Recur runs agents that speak the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP v1, JSON-RPC 2.0 over the agent's stdio) through one generic provider,
`recur-provider-acp` (`providers/acp`). Each agent is a row in
`recur_domain::ACP_AGENTS`:

| Id | Label | Command | Binary override |
|---|---|---|---|
| `antigravity` | Antigravity | `agy acp` | `$RECUR_AGY` |
| `opencode` | OpenCode | `opencode acp` | `$RECUR_OPENCODE` |

Adding an ACP agent is one row there (id, label, binary, args, install and
login hints). Services, the model catalog, Settings and the welcome checklist
iterate over `ACP_AGENTS`; nothing has a per-agent branch. Claude Code, Cursor
and Codex stay on their native protocols.

## Architecture

```
Recur ──ACP v1 JSON-RPC stdio──▶ agy acp | opencode acp
                │                  cwd = worktree
                │                  mcpServers = Recur HTTP MCP (+ user MCP)
                ▼
         AcpProvider::run_turn → normalized events → engine
```

Why ACP rather than agy's `-p --output-format stream-json` CLI mode:

- One client for every ACP agent: Antigravity and OpenCode today, any other
  ACP stdio agent as a registry row.
- Permission requests (`session/request_permission`), session resume and load
  (`session/resume`, `session/load`), image prompts and MCP servers are part of
  the protocol, so none of them are scraped from CLI output or passed as
  agent-specific flags.
- Nothing is written into the worktree: MCP servers ride on `session/new`.

The agent is spawned **per turn** in the worktree, because the Recur MCP URL
(and its token) is per agent. The conversation survives across turns through
the agent's own session store: the resume handle is `<agent id>:<ACP session
id>`, reopened with `session/resume` when the agent advertises it, else
`session/load`.

Agents own their sign-in. Recur stores no ACP credentials; Settings shows the
agent's login hint (`agy`, `opencode auth login`) until a probe session opens.

## Services and Settings

- `AppServices::new` registers one `AcpProvider` per `ACP_AGENTS` entry with
  the data dir and the tracker `env_scrub`, like Codex.
- Model catalog: ACP groups follow Codex, one per agent, only when the agent is
  on and lists models. Live listing probes every enabled agent concurrently;
  the Settings-save fallback uses each provider's cached list first and probes
  ACP agents last.
- `provider_runnable`: on and `locate()` finds the binary. ACP agents count
  toward "some provider offers models" in `recur-domain` (they need no
  Recur-stored config); `ensure_runnable_catalogue` turns Claude Code back on
  when an enabled agent is not installed and nothing else can run.
- `SettingsService::get_acp_status(id)` returns `AcpStatus` (installed,
  binPath, version, authenticated, modelCount, error). Settings → Providers has
  one group per agent after OpenAI: the toggle, Path and Version, and the
  install or login hint while it is not ready.

## Contract mapping

| `TurnRequest` | ACP |
|---|---|
| Worktree | process `cwd` and `session/new` `cwd` |
| Model (`<id>/<model>`) | the part after the first slash, set on the session |
| System prompt | prepended to the first user message (no system-prompt slot) |
| Recur MCP URL | HTTP `mcpServers` entry on `session/new` |
| User MCP servers | HTTP and stdio `mcpServers` entries |
| Resume | `session/resume`, else `session/load` |
| Images | `image` content blocks in `session/prompt` |
| Effort flag | the `thought_level` config option |
| Read-only roles | the agent's `plan` mode when it has one; edit, delete and move requests are rejected |
| Permissions | `session/request_permission` to Recur's permission callback; bypass auto-allows; Recur's own MCP tools never prompt |
| Tool names | MCP calls normalized to `mcp__<server>__<tool>` (agy sends `mcp_<server>_<tool>`, OpenCode `<server>_<tool>`) |
| Cancel | `session/cancel`, then the process group is killed |
| Steer | unsupported |
| Usage | the `session/prompt` `usage` and `usage_update` (context size); dollar cost is not shown |

## Tools Recur replaces

Recur owns asking the human, skills, subagents, todos, background work and
schedules for every provider (Claude Code gets `--disallowedTools`). Each
`ACP_AGENTS` row lists the agent's own tools for those jobs with the Recur
tool to use instead (`blocked_tools`), and `tool_block` says how they are
switched off (`providers/acp/src/launch.rs`). Neither way writes into the
worktree or the user's own agent config.

| Agent | Blocked | How |
|---|---|---|
| OpenCode | `question`, `skill`, `task`, `todowrite`, `todoread` | `OPENCODE_CONFIG_CONTENT` `permission` denies; the tools are removed from the model |
| Antigravity | `ask_question`, `invoke_subagent`, `define_subagent`, `manage_subagents`, `swarm`, `run_workflow`, `send_message`, `manage_task`, `schedule` | A Recur-owned `--gemini-dir` (`<data dir>/acp-home/antigravity`) whose `config/hooks.json` `PreToolUse` hook denies each call with the Recur tool to use |

Antigravity has no switch that hides a tool from the model and keeps MCP
tools (a custom agent's `tools` allowlist drops every MCP tool), so its
blocked tools stay declared and a call fails with the hook's reason. The
Recur-owned home keeps Recur's sessions and probes out of the user's
`~/.gemini` history; sign-in comes from the system keychain, so it needs no
separate login, and the user's own Antigravity skills, plugins and settings
do not load (as with Codex's Recur-owned `CODEX_HOME`).

Tool cards: Antigravity sends its own tool names (`write_file`, `read_file`,
`edit_file`, `run_command`, `search_web`, `invoke_subagent`); OpenCode sends
only an ACP kind and a title, so grep (titled with its pattern) and glob
(titled with its directory) share `kind: search`, and web search is an
`other` call titled "… Web Search: …". `turn::display` maps all of them to
the common cards.

## Limitations

- No mid-turn steer: ACP has no equivalent of Codex `turn/steer`, so a message
  sent during a turn waits for the next one.
- The system prompt rides in the user message, so it is not a Claude-style
  full system-prompt replace and the agent's own instructions still apply.
- Token usage is whatever the agent reports; there is no dollar cost.
- A repo's own agent config (`AGENTS.md`, `opencode.json`) may still load, as
  `CLAUDE.md` does for Claude Code.
