package codex

import (
	"fmt"
	"strings"
	"unicode/utf8"
)

// MCPServer is one MCP server to inject via --config overrides.
type MCPServer struct {
	Name     string // e.g. "recur"
	URL      string // streamable HTTP URL
	Required bool
	// Command/Args/Env for stdio servers (Playwright, custom).
	Command string
	Args    []string
	Env     map[string]string
	// HTTPHeaders for streamable-HTTP servers (Linear, custom). Never log.
	HTTPHeaders map[string]string
}

// ConfigOverrides builds `codex -c key=value` pairs for MCP servers and
// automation-safe defaults. Values are TOML-encoded per Codex docs.
func ConfigOverrides(servers []MCPServer) []string {
	var out []string
	for _, s := range servers {
		name := strings.TrimSpace(s.Name)
		if name == "" {
			continue
		}
		prefix := "mcp_servers." + name
		out = append(out, prefix+`.enabled=true`)
		if s.Required {
			out = append(out, prefix+`.required=true`)
		}
		// Auto-approve tools from Recur/Playwright MCP so automation does not
		// stall on server→client elicitations (still handled as a failsafe).
		out = append(out, prefix+`.default_tools_approval_mode="approve"`)
		if s.URL != "" {
			out = append(out, fmt.Sprintf(`%s.url="%s"`, prefix, escapeTOMLString(s.URL)))
		}
		if s.Command != "" {
			out = append(out, fmt.Sprintf(`%s.command="%s"`, prefix, escapeTOMLString(s.Command)))
			if len(s.Args) > 0 {
				parts := make([]string, len(s.Args))
				for i, a := range s.Args {
					parts[i] = `"` + escapeTOMLString(a) + `"`
				}
				out = append(out, fmt.Sprintf(`%s.args=[%s]`, prefix, strings.Join(parts, ",")))
			}
		}
		for k, v := range s.HTTPHeaders {
			k = strings.TrimSpace(k)
			if k == "" {
				continue
			}
			out = append(out, fmt.Sprintf(`%s.http_headers.%s="%s"`, prefix, k, escapeTOMLString(v)))
		}
		for k, v := range s.Env {
			k = strings.TrimSpace(k)
			if k == "" {
				continue
			}
			out = append(out, fmt.Sprintf(`%s.env.%s="%s"`, prefix, k, escapeTOMLString(v)))
		}
	}
	// Automation defaults — also passable on thread/turn start. Approval policy
	// is appended by CodexRunner (on-request vs never) so this never is only
	// the failsafe if a caller does not override.
	out = append(out,
		`approval_policy="never"`,
		`sandbox_mode="workspace-write"`,
		// Opt into readable reasoning summaries so Recur can stream them as
		// thinking blocks (item/reasoning/summaryTextDelta + completed summary).
		`model_reasoning_summary="auto"`,
		`model_supports_reasoning_summaries=true`,
		// Recur owns skill catalog/activation via MCP. Isolated CODEX_HOME
		// already drops ~/.codex/skills. skip_host_skill_discovery alone does
		// not stop ~/.agents/skills or repo .agents/skills (Codex 0.151 still
		// injected a 20k+ char ## Skills catalog); include_instructions=false
		// drops the catalog from the prompt outright, and remote_plugin=false
		// drops the <recommended_plugins> block. Verified with
		// `codex debug prompt-input -c ...`.
		`skills.bundled.enabled=false`,
		`skills.include_instructions=false`,
		`features.skip_host_skill_discovery=true`,
		`features.remote_plugin=false`,
		// Delegation goes through Recur child workspaces (create_workspace),
		// never Codex's own spawn-agent tools.
		`features.multi_agent=false`,
		`features.multi_agent_v2=false`,
	)
	return out
}

func escapeTOMLString(s string) string {
	var b strings.Builder
	b.Grow(len(s))
	for i := 0; i < len(s); {
		r, size := utf8.DecodeRuneInString(s[i:])
		i += size
		switch r {
		case '\\':
			b.WriteString(`\\`)
		case '"':
			b.WriteString(`\"`)
		case '\b':
			b.WriteString(`\b`)
		case '\t':
			b.WriteString(`\t`)
		case '\n':
			b.WriteString(`\n`)
		case '\f':
			b.WriteString(`\f`)
		case '\r':
			b.WriteString(`\r`)
		default:
			if r < 0x20 {
				fmt.Fprintf(&b, `\u%04x`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	return b.String()
}

// ConfigArgs flattens overrides into interleaved `-c`, `key=value` argv pairs.
func ConfigArgs(overrides []string) []string {
	out := make([]string, 0, len(overrides)*2)
	for _, o := range overrides {
		out = append(out, "-c", o)
	}
	return out
}
