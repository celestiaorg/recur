package domain

import (
	"fmt"
	"regexp"
	"strings"
)

// User MCP catalog (Settings → MCP). Recur MCP and Playwright stay engine-owned.

const (
	MCPIDLinear          = "linear"
	MCPPresetLinear      = "linear"
	MCPTransportHTTP     = "http"
	MCPTransportStdio    = "stdio"
	LinearMCPWriteURL    = "https://mcp.linear.app/mcp"
	LinearMCPReadOnlyURL = "https://mcp.linear.app/mcp/readonly"
)

// MCPServer is one extra MCP server the user added or enabled (a shipped preset).
type MCPServer struct {
	ID        string            `json:"id"`
	Name      string            `json:"name"`
	Preset    string            `json:"preset,omitempty"`
	Enabled   bool              `json:"enabled"`
	Transport string            `json:"transport,omitempty"` // http | stdio
	URL       string            `json:"url,omitempty"`
	Headers   map[string]string `json:"headers,omitempty"`
	Command   string            `json:"command,omitempty"`
	Args      []string          `json:"args,omitempty"`
	Env       map[string]string `json:"env,omitempty"`
}

var mcpIDRe = regexp.MustCompile(`^[a-z][a-z0-9-]{0,40}$`)

// ValidMCPID reports whether id can be an MCP config key (mcp__<id>__*).
// recur and playwright are reserved for engine-owned servers.
func ValidMCPID(id string) bool {
	id = strings.TrimSpace(id)
	if id == "" || id == "recur" || id == "playwright" {
		return false
	}
	return mcpIDRe.MatchString(id)
}

// ValidateMCPServers rejects reserved, malformed, or duplicate ids. Call at the
// settings save boundary so a bad catalog cannot replace a stored server.
func ValidateMCPServers(servers []MCPServer) error {
	seen := map[string]struct{}{}
	for _, s := range servers {
		id := strings.ToLower(strings.TrimSpace(s.ID))
		if !ValidMCPID(id) {
			raw := strings.TrimSpace(s.ID)
			if raw == "" {
				return fmt.Errorf("mcp server id is required")
			}
			return fmt.Errorf("mcp server id %q is reserved or invalid", raw)
		}
		if _, ok := seen[id]; ok {
			return fmt.Errorf("duplicate mcp server id %q", id)
		}
		seen[id] = struct{}{}
	}
	return nil
}

// NormalizeMCPServers drops invalid/reserved ids, forces the Linear preset
// shape, and dedupes by id (last wins). Runtime catalog/enablement only —
// SetSettings must ValidateMCPServers so a bad save cannot clobber storage.
func NormalizeMCPServers(servers []MCPServer) []MCPServer {
	seen := map[string]int{}
	var out []MCPServer
	for _, s := range servers {
		s = normalizeMCPServer(s)
		if s.ID == "" {
			continue
		}
		if i, ok := seen[s.ID]; ok {
			out[i] = s
			continue
		}
		seen[s.ID] = len(out)
		out = append(out, s)
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func normalizeMCPServer(s MCPServer) MCPServer {
	s.ID = strings.ToLower(strings.TrimSpace(s.ID))
	if !ValidMCPID(s.ID) {
		return MCPServer{}
	}
	s.Name = strings.TrimSpace(s.Name)
	s.Preset = strings.TrimSpace(strings.ToLower(s.Preset))
	s.Transport = strings.ToLower(strings.TrimSpace(s.Transport))
	s.URL = strings.TrimSpace(s.URL)
	s.Command = strings.TrimSpace(s.Command)
	if s.Preset == MCPPresetLinear || s.ID == MCPIDLinear {
		s.ID = MCPIDLinear
		s.Preset = MCPPresetLinear
		s.Transport = MCPTransportHTTP
		s.URL = ""
		s.Headers = nil
		s.Command = ""
		s.Args = nil
		s.Env = nil
		if s.Name == "" || strings.EqualFold(s.Name, MCPIDLinear) {
			s.Name = "Linear"
		}
		return s
	}
	if s.Name == "" {
		s.Name = s.ID
	}
	if s.Transport != MCPTransportStdio {
		s.Transport = MCPTransportHTTP
	}
	return s
}

// MCPCatalog is shipped presets ∪ stored servers (stored wins on the same id).
// Linear is always first so Settings/Project can list it without a separate row.
func MCPCatalog(stored []MCPServer) []MCPServer {
	linear := MCPServer{
		ID: MCPIDLinear, Name: "Linear", Preset: MCPPresetLinear, Transport: MCPTransportHTTP,
	}
	var custom []MCPServer
	seenCustom := map[string]int{}
	for _, s := range NormalizeMCPServers(stored) {
		if s.ID == MCPIDLinear {
			linear = s
			continue
		}
		if i, ok := seenCustom[s.ID]; ok {
			custom[i] = s
			continue
		}
		seenCustom[s.ID] = len(custom)
		custom = append(custom, s)
	}
	return append([]MCPServer{linear}, custom...)
}

// MCPGlobalDisabled is the off-list for EffectiveSkillEnabled: shipped presets
// default off; a stored server is off when Enabled is false.
func MCPGlobalDisabled(stored []MCPServer) []string {
	on := map[string]bool{}
	var extraOff []string
	for _, s := range NormalizeMCPServers(stored) {
		if s.Enabled {
			on[s.ID] = true
			continue
		}
		if s.ID != MCPIDLinear {
			extraOff = append(extraOff, s.ID)
		}
	}
	off := make([]string, 0, 1+len(extraOff))
	if !on[MCPIDLinear] {
		off = append(off, MCPIDLinear)
	}
	off = append(off, extraOff...)
	return off
}

// LinearMCPURL is the official Linear MCP endpoint for a chat mode.
// Ask/Plan/Review get the readonly URL so Cursor (no per-run allowlist) cannot
// see write tools. Agent and Loop (Maestro) get the write URL for triage.
func LinearMCPURL(mode string) string {
	switch NormalizeChatMode(mode) {
	case ChatModeAgent, ChatModeLoop:
		return LinearMCPWriteURL
	default:
		return LinearMCPReadOnlyURL
	}
}
