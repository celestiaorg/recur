package engine

import (
	"encoding/json"
	"fmt"
	"strings"

	"recur/internal/cursor"
)

// Cursor's built-in tools have their own names and argument shapes. Recur's
// activity feed, session transcript and tool cards are all keyed off ONE
// canonical vocabulary (which happens to match Claude Code's, since that was the
// first provider), so this file translates at the runner boundary — exactly as
// SDKMessage.ToolLabel already does for MCP tools.
//
// Translating here rather than teaching the UI two dialects means a Cursor
// `shell` call renders in the same terminal card as a Claude `Bash` call, and
// nothing downstream has to know which provider ran the turn.
//
// Shapes below are transcribed from live bridge traffic (internal/cursor probe),
// not from documentation — Cursor publishes neither the tool list nor the arg
// schemas.

// canonicalTool maps a Cursor tool name to Recur's vocabulary. Unknown names
// pass through unchanged: the feed falls back to the raw name and a generic
// icon, which is a fair degradation for a tool Cursor added after this build.
func canonicalTool(name string) string {
	switch name {
	case "shell":
		return "Bash"
	case "read":
		return "Read"
	case "edit":
		return "Edit"
	case "write":
		return "Write"
	case "glob":
		return "Glob"
	case "grep":
		return "Grep"
	case "delete":
		return "Delete"
	case "updateTodos":
		return "TodoWrite"
	case "task":
		return "Task"
	case "createPlan":
		// Cursor plan-mode's create-plan tool. Args carry the plan markdown;
		// the result is an empty ack (same pattern as updateTodos).
		return "CreatePlan"
	default:
		return name
	}
}

// cursorToolInput rewrites a Cursor tool's arguments into the canonical input
// object the tool cards read (`command`, `file_path`, `pattern`, …). Returns nil
// when there is nothing useful to show, which the cards render as a bare row.
func cursorToolInput(name string, raw json.RawMessage) json.RawMessage {
	if len(raw) == 0 {
		return nil
	}
	var a map[string]any
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil
	}
	str := func(k string) string {
		v, _ := a[k].(string)
		return v
	}

	out := map[string]any{}
	switch name {
	case "shell":
		// Cursor also sends `timeout`, which is plumbing, not worth a chip.
		if c := str("command"); c != "" {
			out["command"] = c
		}
	case "read", "edit", "delete":
		if p := str("path"); p != "" {
			out["file_path"] = p
		}
	case "glob":
		if p := str("globPattern"); p != "" {
			out["pattern"] = p
		}
		if d := str("targetDirectory"); d != "" {
			out["path"] = d
		}
	case "grep":
		if p := str("pattern"); p != "" {
			out["pattern"] = p
		}
		if p := str("path"); p != "" {
			out["path"] = p
		}
	case "updateTodos":
		if t, ok := a["todos"]; ok {
			out["todos"] = normalizeTodos(t)
		}
	case "task":
		// Same shape Claude Code's Task uses: a one-line description (row title),
		// the prompt the subagent runs, and optional type/model. Plumbing fields
		// stay out of the card.
		if d := str("description"); d != "" {
			out["description"] = d
		}
		if p := str("prompt"); p != "" {
			out["prompt"] = p
		}
		if t := str("subagent_type"); t != "" {
			out["subagent_type"] = t
		}
		if m := str("model"); m != "" {
			out["model"] = m
		}
	case "createPlan":
		// Plan text lives in args; result is {}. Keep unknown keys out of the
		// card so a future Cursor field does not dump as raw JSON noise.
		if p := str("plan"); p != "" {
			out["plan"] = p
		}
	default:
		// An unmapped tool keeps its arguments verbatim — better a raw JSON card
		// than an empty one.
		return raw
	}
	if len(out) == 0 {
		return nil
	}
	b, err := json.Marshal(out)
	if err != nil {
		return nil
	}
	return b
}

// cursorToolResultText renders a completed Cursor tool call as the result text
// the transcript and tool cards display.
//
// Cursor nests everything under {"status":…,"value":{…}} with a per-tool shape,
// so without this the mirror records an empty result and every tool card shows a
// call with no output.
func cursorToolResultText(name string, raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var env struct {
		Status string          `json:"status"`
		Value  json.RawMessage `json:"value"`
	}
	if err := json.Unmarshal(raw, &env); err != nil || len(env.Value) == 0 {
		return ""
	}

	var v map[string]any
	if err := json.Unmarshal(env.Value, &v); err != nil {
		return ""
	}
	str := func(k string) string {
		s, _ := v[k].(string)
		return s
	}
	num := func(k string) (float64, bool) {
		f, ok := v[k].(float64)
		return f, ok
	}

	switch name {
	case "shell":
		// Mirror what a terminal shows: stdout, then stderr, then a non-zero exit.
		var b strings.Builder
		b.WriteString(str("stdout"))
		if e := str("stderr"); e != "" {
			if b.Len() > 0 {
				b.WriteString("\n")
			}
			b.WriteString(e)
		}
		if code, ok := num("exitCode"); ok && code != 0 {
			if b.Len() > 0 {
				b.WriteString("\n")
			}
			fmt.Fprintf(&b, "exit %d", int(code))
		}
		return b.String()

	case "read":
		return str("content")

	case "edit":
		// Cursor puts the edit's diff in the RESULT; its args carry only the path,
		// so this is the only place the change itself appears.
		return str("diffString")

	case "glob":
		files, _ := v["files"].([]any)
		var lines []string
		for _, f := range files {
			if s, ok := f.(string); ok {
				lines = append(lines, s)
			}
		}
		return strings.Join(lines, "\n")

	case "grep":
		return grepResultText(v)

	case "delete":
		return ""

	case "updateTodos":
		return ""

	case "task":
		return taskResultText(env.Value)

	case "createPlan":
		// Ack only — the plan body is in the tool args (see createPlanBody).
		return ""

	case cursor.ToolNameMCP:
		return mcpResultText(env.Value)
	}

	// Unknown tool: hand back the raw value rather than dropping the output.
	return truncate(strings.TrimSpace(string(env.Value)), feedInputLimit)
}

// taskResultText pulls the subagent's return text out of Cursor's Task result
// value. The shape is not documented; try the common string fields first, then
// fall back to a compact JSON dump so the card is never empty when the provider
// sent something.
func taskResultText(value json.RawMessage) string {
	if len(value) == 0 {
		return ""
	}
	// Plain string value.
	var s string
	if err := json.Unmarshal(value, &s); err == nil {
		return s
	}
	var v map[string]any
	if err := json.Unmarshal(value, &v); err != nil {
		return truncate(strings.TrimSpace(string(value)), feedInputLimit)
	}
	str := func(k string) string {
		s, _ := v[k].(string)
		return s
	}
	for _, k := range []string{"text", "result", "output", "message", "content"} {
		if s := str(k); s != "" {
			return s
		}
	}
	return truncate(strings.TrimSpace(string(value)), feedInputLimit)
}

// createPlanBody extracts the plan markdown from a createPlan / CreatePlan tool
// input. Empty when the payload is missing, malformed, or not yet filled in
// (Cursor often streams a first running frame with {"plan":""}).
func createPlanBody(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var a struct {
		Plan string `json:"plan"`
	}
	if err := json.Unmarshal(raw, &a); err != nil {
		return ""
	}
	return strings.TrimSpace(a.Plan)
}

// isCreatePlanTool reports whether name is Cursor's create-plan tool in either
// the raw provider spelling or the common vocabulary.
func isCreatePlanTool(name string) bool {
	return name == "createPlan" || name == "CreatePlan"
}

// normalizeTodos rewrites Cursor's todo statuses into the common vocabulary the
// todo checklist renders (pending | in_progress | completed).
//
// Cursor sends camelCase "inProgress"; Claude Code's TodoWrite sends
// "in_progress". Everything else already matches, and an unrecognised status
// passes through so a new one shows as-is rather than silently reading as
// pending.
func normalizeTodos(v any) any {
	items, ok := v.([]any)
	if !ok {
		return v
	}
	out := make([]any, 0, len(items))
	for _, it := range items {
		m, ok := it.(map[string]any)
		if !ok {
			out = append(out, it)
			continue
		}
		cp := make(map[string]any, len(m))
		for k, val := range m {
			cp[k] = val
		}
		if s, ok := cp["status"].(string); ok && s == "inProgress" {
			cp["status"] = "in_progress"
		}
		out = append(out, cp)
	}
	return out
}

// grepResultText flattens Cursor's nested grep payload
// (workspaceResults → <dir> → output.matches[].file) into a file list.
func grepResultText(v map[string]any) string {
	ws, _ := v["workspaceResults"].(map[string]any)
	var lines []string
	for _, res := range ws {
		r, _ := res.(map[string]any)
		out, _ := r["output"].(map[string]any)
		matches, _ := out["matches"].([]any)
		for _, m := range matches {
			mm, _ := m.(map[string]any)
			if f, ok := mm["file"].(string); ok {
				lines = append(lines, f)
			}
		}
	}
	return strings.Join(lines, "\n")
}

// mcpResultText pulls the text out of an MCP tool result, whose content items
// wrap their text one level deeper than the MCP spec's own shape:
// {"content":[{"text":{"text":"…"}}]}.
func mcpResultText(value json.RawMessage) string {
	var v struct {
		Content []struct {
			Text struct {
				Text string `json:"text"`
			} `json:"text"`
		} `json:"content"`
	}
	if err := json.Unmarshal(value, &v); err != nil {
		return ""
	}
	var parts []string
	for _, c := range v.Content {
		if c.Text.Text != "" {
			parts = append(parts, c.Text.Text)
		}
	}
	return strings.Join(parts, "\n")
}
