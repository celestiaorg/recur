package agenttools

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestAskUserAndSwitchModeHalt(t *testing.T) {
	names := map[string]bool{}
	for _, tool := range HostTools() {
		names[tool.Name()] = true
		if Halts(tool) != HaltNames()[tool.Name()] {
			t.Errorf("%s Halts() = %v, HaltNames = %v", tool.Name(), Halts(tool), HaltNames()[tool.Name()])
		}
	}
	if !names["create_workspace"] {
		t.Error("HostTools missing create_workspace")
	}
	if !names["message_child"] {
		t.Error("HostTools missing message_child")
	}
}

func TestMessageChildSchemaRequiresWorkspaceAndMessage(t *testing.T) {
	var tool Tool
	for _, tl := range HostTools() {
		if tl.Name() == "message_child" {
			tool = tl
			break
		}
	}
	if tool == nil {
		t.Fatal("HostTools missing message_child")
	}
	var schema struct {
		Required []string `json:"required"`
	}
	if err := json.Unmarshal(tool.Schema(), &schema); err != nil {
		t.Fatalf("schema: %v", err)
	}
	want := map[string]bool{"workspace_id": false, "message": false}
	for _, r := range schema.Required {
		if _, ok := want[r]; ok {
			want[r] = true
		}
	}
	for field, ok := range want {
		if !ok {
			t.Errorf("message_child schema required missing %q; required=%v", field, schema.Required)
		}
	}
}

func TestCreateWorkspaceDescriptionPrefersChild(t *testing.T) {
	d := createWorkspaceTool{}.Description()
	if !strings.Contains(d, "Prefer a child") {
		t.Errorf("parent guidance missing: %q", d)
	}
}
