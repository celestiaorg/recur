# OpenAI-compatible LLM provider

For browser-based access through a ChatGPT plan, see the
[direct ChatGPT provider](chatgpt-provider.md).

Recur can run interactive chats and loops against any HTTP endpoint that
implements OpenAI's **`POST /v1/responses`** API, or only
**`POST /v1/chat/completions`** (DeepSeek, many local servers; see
[Chat Completions endpoints](#chat-completions-endpoints)). This is a first-class Recur
coding harness: Recur owns the agent loop, the coding tools, and the Recur
host tools. Claude Code, Cursor, and Codex still own *their* loops.

You can add **multiple** compatible providers. Each has its own slug, models,
and flag dimensions.

## Architecture

```
Recur  ──POST {baseURL}/v1/responses──▶  OpenAI / OpenRouter / Groq / …
          │
          │  Recur-owned tool loop
          │  Host tools in-process (no MCP HTTP)
          │  Coding tools: Read, Edit, Write, Glob, Grep, Bash, TodoWrite
          ▼
   HarnessRunner → ParsedEvent / Result → withMirroring
```

Why Recur owns the loop:

- Compatible endpoints are a completions API, not an agent runtime
- Coding tools stay sandboxed to the worktree
- Recur MCP tools (`ask_user`, findings, plans, …) run in-process: the same
  `Execute` path MCP HTTP wraps for Claude/Cursor/Codex
- Native turns skip MCP HTTP (`registerMCP` returns empty)

Out of scope for v1: `/v1/chat/completions`, Playwright, web search, usage
meters, price tables.

## Code map

| File | Role |
|---|---|
| `crates/domain/src/compatible.rs` | Types, slug validation, flag presets, enablement |
| `providers/compatible/src/client.rs`, `sse.rs`, `flags.rs` | HTTP client, SSE parser, flag wiring onto the request body |
| `crates/agent-tools` | First-class Recur tools (host and coding tools); MCP wraps the same `execute` |
| `crates/mcp/src/server.rs` | MCP HTTP wrap of the host tools |
| `providers/compatible/src/harness.rs`, `tools.rs` | The harness runner: Responses API ⇄ Recur tool loop |
| `providers/compatible/src/prompt.rs` | Recur-owned coding system prompt |
| `providers/compatible/src/status.rs` | Settings Test / flag presets |
| `providers/compatible/src/catalog.rs` | One picker group per enabled+configured provider |

## Selecting a provider

There is no per-role provider setting. Role models are `ModelRef`s
(`openai/gpt-5`, `openrouter/openai/gpt-4o`). A compatible provider appears in
pickers when it is on in Settings → Providers **and** configured (base URL +
at least one model). An API key is optional (local servers).

`ParseModelRef` splits on the **first** `/`, so OpenRouter ids with a slash
work: `openrouter/openai/gpt-4o` → provider `openrouter`, id `openai/gpt-4o`.

Reserved slugs (cannot be used for a compatible endpoint): `claude-code`,
`cursor`, `codex`, `chatgpt`, `anthropic`, `gemini`, `ollama`. **`openai` is allowed.**

## Settings

Settings → Providers → **Add provider**. Presets:

The OpenAI preset is configured under **OpenAI → API endpoint**, alongside
ChatGPT account sign-in and the Codex runtime. Their models share one **OpenAI**
picker section; runtime labels distinguish duplicate models when necessary.

| Preset | Default base URL |
|---|---|
| OpenAI | `https://api.openai.com` |
| OpenRouter | `https://openrouter.ai/api` |
| Groq | `https://api.groq.com/openai` |
| Custom | (you fill in) |

Per card: enable toggle, display name, base URL, API key, extra headers, models
(with optional picker controls: Effort, Effort + verbosity, or Custom to edit each
control and its options), Test, Remove.

**Test** POSTs a tiny `store:false` request to `{base}/v1/responses` and a
tiny non-streamed request to `{base}/v1/chat/completions`, both with the first
model id, and reports which answered (`CompatibleTestResult.api`) plus the
harnesses that API set enables (`harnesses`). A passing Test records the API
on the saved card (`CompatibleProvider.api`) when the id and base URL match;
it saves nothing else.

Flag dimensions map onto allowlisted JSON paths on the Responses body:

| Wire | Field |
|---|---|
| `reasoning.effort` | `reasoning.effort` |
| `reasoning.summary` | `reasoning.summary` |
| `text.verbosity` | `text.verbosity` |
| `temperature` | `temperature` |
| `max_output_tokens` | `max_output_tokens` |

The OpenAI preset is effort + verbosity. Unknown wires are dropped at save.

## Contract mapping

| AgentSpec | Compatible harness |
|---|---|
| Worktree | Coding-tool sandbox (`safe_join` + symlink check) |
| Model | Bare id on `POST /v1/responses` |
| Flags | `apply_flags` via each model's `wire` paths |
| Thinking | `reasoning*` SSE deltas → `AgentEvent::Thinking` / `ThinkingDelta` |
| Coding tools | Canonical Claude names so the chat's tool cards work |
| Host tools | In-process `recur_agent_tools::Tool` (same `execute` as MCP HTTP) |
| Environment | Worktree path, platform, and date appended to the base prompt |
| Repo rules | Worktree `AGENTS.md` (else `CLAUDE.md`) inlined, 32 KiB cap, regular files only |
| Ask mode | Allowlist omits Edit/Write/Bash |
| `ask_user` / `switch_mode` | Halt the Responses loop after a successful call |
| Session | `ContextModeApp`: rebuild input from the Recur transcript |
| `fresh_session` | Cutoff at `nativeHistoryAfter`; later turns replay only since the reset |
| Mode remount | Hidden continue note; original user turn (and images) stay in input |
| Chat fork | No provider-transcript inject; history is already in the store (cutoff cleared) |
| MCP HTTP | Not registered for compatible slugs |

## What this is not

This is **not** wrapping Claude, Cursor, or Codex. Those providers still spawn
their own agent runtimes. A compatible endpoint is a model API; Recur is the
agent.

## Chat Completions endpoints

`CompatibleProvider.api` (`responses`, `chat-completions`, `both`, or absent
for untested) picks the route for turns:

| `api` | Route |
|---|---|
| `chat-completions` | Always `/v1/chat/completions` |
| `responses`, `both` | Always `/v1/responses` |
| untested | `/v1/responses`; on a 404 or 405, the same round over `/v1/chat/completions`. A success is remembered for the provider until its base URL changes |

The harness loop is unchanged: `chat::chat_body` rewrites the Responses body
(instructions as a system message, a batch of function calls as one assistant
message followed by its tool results, tools as `{type, function}`,
`max_output_tokens` as `max_tokens`, `reasoning.effort` as `reasoning_effort`,
`text.verbosity` as `verbosity`, `stream_options.include_usage`), and
`chat::ChatStream` turns the chunks back into Responses events (`content` and
`reasoning_content` deltas, tool calls assembled by index, a terminal
`response.completed`, or `response.incomplete` for `finish_reason: length`).
A stream without a finish reason has no terminal event and fails the round.

The model picker uses the same field: `compatible_harness_blocker` in
`recur_domain` disables OpenCode for a `responses` endpoint, since OpenCode
only speaks Chat Completions.
