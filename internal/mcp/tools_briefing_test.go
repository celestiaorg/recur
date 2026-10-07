package mcp

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestBriefingToolsRefuseNonAPP(t *testing.T) {
	s := New(&captureReporter{})
	b := binding{agentCode: "MA"}
	for _, name := range []string{toolListWorkspaceBriefs, toolGetWorkspaceContext, toolPresentWorkspace} {
		res := call(t, s, b, name, map[string]any{
			"workspace_id": "WS-1",
			"target":       "workspace:WS-1",
		})
		if !res.IsError {
			t.Errorf("%s: expected tool-level error for non-APP agent", name)
			continue
		}
		if len(res.Content) == 0 || !strings.Contains(res.Content[0].Text, "Ask Recur") {
			t.Errorf("%s: error should name Ask Recur, got %+v", name, res)
		}
	}
}

func TestBriefingToolsAPPHappyPath(t *testing.T) {
	s := New(&captureReporter{})
	b := binding{agentCode: domain.AppAgentCode}

	list := call(t, s, b, toolListWorkspaceBriefs, map[string]any{})
	if list.IsError {
		t.Fatalf("list_workspace_briefs: %+v", list)
	}
	if len(list.Content) == 0 || !strings.Contains(list.Content[0].Text, "WS-1") {
		t.Fatalf("list_workspace_briefs body: %+v", list)
	}

	ctx := call(t, s, b, toolGetWorkspaceContext, map[string]any{"workspace_id": "WS-9"})
	if ctx.IsError {
		t.Fatalf("get_workspace_context: %+v", ctx)
	}
	if len(ctx.Content) == 0 || !strings.Contains(ctx.Content[0].Text, "WS-9") {
		t.Fatalf("get_workspace_context body: %+v", ctx)
	}

	present := call(t, s, b, toolPresentWorkspace, map[string]any{"target": "workspace:WS-1"})
	if present.IsError {
		t.Fatalf("present_workspace: %+v", present)
	}
	if len(present.Content) == 0 || !strings.Contains(present.Content[0].Text, "workspace:WS-1") {
		t.Fatalf("present_workspace body: %+v", present)
	}
}
