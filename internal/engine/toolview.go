package engine

import (
	"encoding/json"

	"recur/internal/cursor"
	"recur/internal/domain"
)

// The display boundary for tool calls.
//
// Two rules, and they pull in opposite directions:
//
//  1. The session store is the SOURCE OF TRUTH and holds whatever the provider
//     actually sent — Cursor's `shell` with `{command, timeout}` and its nested
//     `{"status":…,"value":{…}}` result, not a rewritten version. A transcript
//     that silently edits provider data is no longer a record of what happened,
//     and it would make a provider's own quirks undebuggable from the DB.
//
//  2. The UI shows ONE vocabulary across providers, so a Cursor `shell` call and
//     a Claude `Bash` call render in the same terminal card and users do not
//     have to learn two dialects.
//
// So translation lives here, at the read/render boundary, and never on the way
// in. Everything below is a pure function of (provider, raw) — nothing is
// persisted, so changing a mapping re-renders old transcripts correctly instead
// of leaving them stale.

// toolDisplay is a provider's tool call rendered in the common vocabulary.
type toolDisplay struct {
	Name  string          // common name, e.g. Bash
	Input json.RawMessage // common input shape, e.g. {"command":…}
}

// displayTool renders a raw provider tool call for the UI.
func displayTool(provider, name string, input json.RawMessage) toolDisplay {
	switch provider {
	case domain.ProviderCursor:
		// MCP tools already carry a cross-provider name; only their routing
		// envelope needs unwrapping.
		if common, params, ok := cursorMCPDisplay(name, input); ok {
			return toolDisplay{Name: common, Input: params}
		}
		return toolDisplay{Name: canonicalTool(name), Input: cursorToolInput(name, input)}
	case domain.ProviderCodex:
		// CodexRunner already emits mcp__server__tool names; shell/edit map
		// through the same canonical vocabulary as Cursor.
		return toolDisplay{Name: canonicalTool(name), Input: codexToolInput(name, input)}
	default:
		// Claude Code defined most of the common vocabulary, so its calls pass
		// through. Agent is the exception: it was renamed from Task in v2.1.63,
		// and Recur's subagent card is still keyed on Task.
		return toolDisplay{Name: canonicalClaudeTool(name), Input: input}
	}
}

// canonicalClaudeTool maps Claude Code tool names that drifted from Recur's
// vocabulary. Unknown names pass through unchanged.
func canonicalClaudeTool(name string) string {
	switch name {
	case "Agent":
		return "Task"
	default:
		return name
	}
}

// displayToolResult renders a raw provider tool result as the text the cards and
// transcript show.
func displayToolResult(provider, name, raw string) string {
	var out string
	switch provider {
	case domain.ProviderCursor:
		out = cursorToolResultText(name, json.RawMessage(raw))
	case domain.ProviderCodex:
		out = raw
	default:
		out = raw
	}
	if isTaskTool(name) && isSubagentLaunchMeta(out) {
		return ""
	}
	return out
}

// codexToolInput maps Codex tool args into the common card shape.
func codexToolInput(name string, raw json.RawMessage) json.RawMessage {
	if len(raw) == 0 {
		return nil
	}
	var a map[string]any
	if err := json.Unmarshal(raw, &a); err != nil {
		return raw
	}
	str := func(k string) string {
		v, _ := a[k].(string)
		return v
	}
	out := map[string]any{}
	switch name {
	case "shell":
		if c := str("command"); c != "" {
			out["command"] = c
		}
	case "edit", "write", "Write", "read", "delete":
		if p := str("path"); p != "" {
			out["file_path"] = p
		} else if p := str("file_path"); p != "" {
			out["file_path"] = p
		}
		if name == "write" || name == "Write" {
			if c := str("content"); c != "" {
				out["content"] = c
			}
		}
	default:
		return raw
	}
	if len(out) == 0 {
		return raw
	}
	b, err := json.Marshal(out)
	if err != nil {
		return raw
	}
	return b
}

// displayToolFor renders the tool call on a ParsedEvent.
func displayToolFor(ev ParsedEvent) toolDisplay {
	return displayTool(ev.Provider, ev.Tool, ev.ToolInputRaw)
}

// summarizeDisplayInput renders the feed's one-line tool summary from a raw
// provider input, via the common shape so the wording matches across providers.
func summarizeDisplayInput(provider, name string, input json.RawMessage) string {
	d := displayTool(provider, name, input)
	return summarizeToolInput(d.Name, d.Input)
}

// cursorMCPDisplay unwraps Cursor's MCP routing envelope
// ({providerIdentifier, toolName, args}) into the cross-provider
// `mcp__<server>__<tool>` name plus the tool's own parameters. ok is false for
// anything that is not an MCP call.
func cursorMCPDisplay(name string, input json.RawMessage) (string, json.RawMessage, bool) {
	if name != cursor.ToolNameMCP {
		return "", nil, false
	}
	var a cursor.ToolCallArgs
	if err := json.Unmarshal(input, &a); err != nil || a.ToolName == "" {
		return name, input, true
	}
	common := "mcp__" + a.ToolName
	if a.ProviderIdentifier != "" {
		common = "mcp__" + a.ProviderIdentifier + "__" + a.ToolName
	}
	return common, a.Params, true
}
