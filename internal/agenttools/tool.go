package agenttools

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
)

// RecurMCPPrefix is how runner allowlists name Recur host tools.
const RecurMCPPrefix = "mcp__recur__"

// Tool is a first-class Recur tool. MCP HTTP wraps Execute; the native harness
// calls it in-process.
type Tool interface {
	Name() string
	Description() string
	Schema() json.RawMessage
	Execute(Call) (string, error)
}

// HaltTool is implemented by host tools that end the calling turn (ask_user,
// switch_mode). The harness stops the model loop after a successful Execute.
type HaltTool interface {
	Halts() bool
}

// Permitter asks the human whether a mutating tool may run.
type Permitter interface {
	RequestPermission(ctx context.Context, workspaceID, agentCode, tool string, input json.RawMessage) (allow bool, reason string, err error)
}

// Call is one tool invocation.
type Call struct {
	Ctx         context.Context
	WorkspaceID string
	AgentCode   string
	// ChatCode is the conductor tab (PA*, MA) for a loop specialist turn.
	// Empty for interactive chats, where AgentCode is already the tab.
	ChatCode    string
	Worktree    string
	SessionID   string
	Args        json.RawMessage
	BypassPerms bool
	// Host is the engine callback seam. Nil for coding-only calls.
	Host Host
	// Permitter gates Bash when BypassPerms is false.
	Permitter Permitter
	Todos     *TodoStore
}

// Registry maps short tool names to implementations.
type Registry struct {
	mu    sync.RWMutex
	tools map[string]Tool
	order []string
}

// NewRegistry builds a registry from tools. Later tools with the same name replace earlier ones.
func NewRegistry(tools ...Tool) *Registry {
	r := &Registry{tools: map[string]Tool{}}
	for _, t := range tools {
		if t == nil || t.Name() == "" {
			continue
		}
		r.Add(t)
	}
	return r
}

// Add registers t by Name().
func (r *Registry) Add(t Tool) {
	if r == nil || t == nil {
		return
	}
	name := t.Name()
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.tools == nil {
		r.tools = map[string]Tool{}
	}
	if _, ok := r.tools[name]; !ok {
		r.order = append(r.order, name)
	}
	r.tools[name] = t
}

// Get returns the tool with the given short or mcp-prefixed name.
func (r *Registry) Get(name string) (Tool, bool) {
	if r == nil {
		return nil, false
	}
	name = BareName(name)
	r.mu.RLock()
	defer r.mu.RUnlock()
	t, ok := r.tools[name]
	return t, ok
}

// List returns tools in registration order, optionally filtered by an allowlist
// of runner names (Read, mcp__recur__ask_user, …). nil allowlist means all.
func (r *Registry) List(allowed []string) []Tool {
	if r == nil {
		return nil
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	if allowed == nil {
		out := make([]Tool, 0, len(r.order))
		for _, name := range r.order {
			out = append(out, r.tools[name])
		}
		return out
	}
	want := map[string]bool{}
	for _, a := range allowed {
		if a == "" {
			continue
		}
		want[a] = true
		want[BareName(a)] = true
	}
	out := make([]Tool, 0, len(r.order))
	for _, name := range r.order {
		if !want[name] && !want[RecurMCPPrefix+name] {
			continue
		}
		out = append(out, r.tools[name])
	}
	return out
}

// AdvertisedName is the string the model should see for t given the turn allowlist.
func AdvertisedName(t Tool, allowed []string) string {
	if t == nil {
		return ""
	}
	short := t.Name()
	prefixed := RecurMCPPrefix + short
	for _, a := range allowed {
		if a == prefixed {
			return prefixed
		}
		if a == short {
			return short
		}
	}
	return short
}

// BareName strips mcp__recur__ if present.
func BareName(name string) string {
	if n, ok := strings.CutPrefix(name, RecurMCPPrefix); ok {
		return n
	}
	return name
}

// Halts reports whether a successful call of this tool should end the turn.
func Halts(t Tool) bool {
	h, ok := t.(HaltTool)
	return ok && h.Halts()
}

// HaltNames are host tools that park/remount the turn.
func HaltNames() map[string]bool {
	return map[string]bool{"ask_user": true, "switch_mode": true}
}

// HaltAfter reports whether a successful Execute should end the native loop.
// switch_mode only halts when the host actually parked/remounted the turn.
func HaltAfter(name, result string) bool {
	switch BareName(name) {
	case "ask_user":
		return true
	case "switch_mode":
		return strings.Contains(result, "remounting")
	default:
		return false
	}
}
