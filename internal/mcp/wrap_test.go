package mcp

import (
	"testing"

	"recur/internal/agenttools"
)

func TestAllToolsWrapsHostTools(t *testing.T) {
	host := agenttools.HostTools()
	specs := allTools()
	if len(specs) != len(host) {
		t.Fatalf("allTools() len = %d, want HostTools %d", len(specs), len(host))
	}
	for i, t0 := range host {
		if specs[i].name != t0.Name() {
			t.Errorf("allTools()[%d].name = %q, HostTools()[%d].Name() = %q", i, specs[i].name, i, t0.Name())
		}
	}
}
