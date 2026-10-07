# OpenAI-compatible LLM provider

Recur can run interactive chats and loops against any HTTP endpoint that
implements OpenAI's **`POST /v1/responses`** API. This is a first-class Recur
coding harness: Recur owns the agent loop, the coding tools, and the Recur
host tools. Claude Code, Cursor, and Codex still own *their* loops.

You can add **multiple** compatible providers. Each has its own slug, models,
and flag dimensions.

## Architecture

```
Recur (Go)  ──POST {baseURL}/v1/responses──▶  OpenAI / OpenRouter / Groq / …
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
- Recur MCP tools (`ask_user`, findings, plans, …) run in-process — the same
  `Execute` path MCP HTTP wraps for Claude/Cursor/Codex
- Native turns skip MCP HTTP (`registerMCP` returns empty)

Out of scope for v1: `/v1/chat/completions`, Playwright, web search, usage
meters, price tables.

## Code map

| File | Role |
|---|---|
| `internal/domain/compatible_provider.go` | Types, slug validation, flag presets, enablement |
| `internal/responses` | HTTP client, SSE parser, flag wiring onto the request body |
| `internal/agenttools` | First-class Recur tools (`HostTools`, `CodingTools`); MCP wraps `Execute` |
| `internal/mcp/tools.go` | MCP HTTP wrap of `agenttools.HostTools()` |
| `internal/engine/harness.go` | `HarnessRunner` — Responses API ⇄ Recur tool loop |
| `internal/engine/harness_prompt.go` | Recur-owned coding system prompt |
| `internal/engine/provider.go` | Compatible slug routing through `runnerFor` |
| `internal/app/compatible_service.go` | Settings Test / flag presets |
| `internal/app/model_catalog.go` | One picker group per enabled+configured provider |

## Selecting a provider

There is no per-role provider setting. Role models are `ModelRef`s
(`openai/gpt-5`, `openrouter/openai/gpt-4o`). A compatible provider appears in
pickers when it is on in Settings → Providers **and** configured (base URL +
at least one model). An API key is optional (local servers).

`ParseModelRef` splits on the **first** `/`, so OpenRouter ids with a slash
work: `openrouter/openai/gpt-4o` → provider `openrouter`, id `openai/gpt-4o`.

Reserved slugs (cannot be used for a compatible endpoint): `claude-code`,
`cursor`, `codex`, `anthropic`, `gemini`, `ollama`. **`openai` is allowed.**

## Settings

Settings → Providers → **Add provider**. Presets:

| Preset | Default base URL |
|---|---|
| OpenAI | `https://api.openai.com` |
| OpenRouter | `https://openrouter.ai/api` |
| Groq | `https://api.groq.com/openai` |
| Custom | (you fill in) |

Per card: enable toggle, display name, base URL, API key, extra headers, models
(with optional Effort / Effort+verbosity flags), Test, Remove.

**Test** POSTs a tiny `store:false` request to `{base}/v1/responses` using the
first model id. It does not persist settings.

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
| Worktree | Coding-tool sandbox (`SafeJoin` + symlink check) |
| Model | Bare id on `POST /v1/responses` |
| Flags | `ApplyFlags` via each model's `Wire` paths |
| Thinking | `reasoning*` SSE deltas → `KindThinking` / `KindThinkingDelta` |
| Coding tools | Canonical Claude names so `toolview.go` cards work |
| Host tools | In-process `agenttools.Tool` (same Execute as MCP HTTP) |
| Ask mode | Allowlist omits Edit/Write/Bash |
| `ask_user` / `switch_mode` | Halt the Responses loop after a successful call |
| Session | `ContextModeApp` — rebuild input from the Recur transcript |
| `fresh_session` | Cutoff at `nativeHistoryAfter`; later turns replay only since the reset |
| Mode remount | Hidden continue note; original user turn (and images) stay in input |
| Chat fork | No provider-transcript inject; history is already in the store (cutoff cleared) |
| MCP HTTP | Not registered for compatible slugs |

## What this is not

This is **not** wrapping Claude, Cursor, or Codex. Those providers still spawn
their own agent runtimes. A compatible endpoint is a model API; Recur is the
agent.
