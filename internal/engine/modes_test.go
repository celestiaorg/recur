package engine

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestChatModeShapeAllowsArtifacts(t *testing.T) {
	set := domain.Settings{}
	artifactTools := []string{mcpCreateArtifact, mcpListArtifacts, mcpReadArtifact, mcpPresentArtifact}
	editTools := []string{"Edit", "Write", "NotebookEdit"}

	for _, mode := range []string{
		domain.ChatModeAsk, domain.ChatModePlan, domain.ChatModeReview, domain.ChatModeAgent,
	} {
		shape := chatModeShape(mode, set, false)
		for _, tool := range artifactTools {
			if !shapeHasTool(shape.tools, tool) {
				t.Errorf("chatModeShape(%q) tools missing %s; tools = %v", mode, tool, shape.tools)
			}
		}
		if mode == domain.ChatModeAgent {
			continue
		}
		for _, tool := range editTools {
			if shapeHasTool(shape.tools, tool) {
				t.Errorf("chatModeShape(%q) must not allow %s; tools = %v", mode, tool, shape.tools)
			}
		}
		if !strings.Contains(shape.appendPrompt, "create_artifact") {
			t.Errorf("chatModeShape(%q).appendPrompt missing create_artifact, got %q", mode, shape.appendPrompt)
		}
		if !strings.Contains(shape.appendPrompt, "git worktree") {
			t.Errorf("chatModeShape(%q).appendPrompt should say git worktree is read-only, got %q", mode, shape.appendPrompt)
		}
		prefer := strings.Contains(shape.appendPrompt, "Prefer an HTML artifact")
		beside := strings.Contains(shape.appendPrompt, "in addition to, never instead of")
		switch mode {
		case domain.ChatModeAsk:
			if !prefer {
				t.Errorf("chatModeShape(%q).appendPrompt should prefer HTML artifacts, got %q", mode, shape.appendPrompt)
			}
			if beside {
				t.Errorf("chatModeShape(%q).appendPrompt should not carry the Plan/Review contract qualifier, got %q", mode, shape.appendPrompt)
			}
		case domain.ChatModePlan, domain.ChatModeReview:
			if prefer {
				t.Errorf("chatModeShape(%q).appendPrompt must not prefer HTML artifacts over structured output, got %q", mode, shape.appendPrompt)
			}
			if !beside {
				t.Errorf("chatModeShape(%q).appendPrompt should say artifacts never replace structured output, got %q", mode, shape.appendPrompt)
			}
		}
	}
}

func TestChatModeShapeAgentPrefersChildWorkspaces(t *testing.T) {
	shape := chatModeShape(domain.ChatModeAgent, domain.Settings{}, false)
	if !strings.Contains(shape.appendPrompt, "beyond a very simple task") {
		t.Errorf("agent appendPrompt should strongly prefer child workspaces, got %q", shape.appendPrompt)
	}
	if !strings.Contains(shape.appendPrompt, mcpCreateWorkspace) {
		t.Errorf("agent appendPrompt should name create_workspace, got %q", shape.appendPrompt)
	}
	child := chatModeShape(domain.ChatModeAgent, domain.Settings{}, true)
	if strings.Contains(child.appendPrompt, "beyond a very simple task") {
		t.Errorf("child agent must not inherit parent delegation preference, got %q", child.appendPrompt)
	}
	if !strings.Contains(child.appendPrompt, "execute the assigned task here") {
		t.Errorf("child agent should be told to execute locally, got %q", child.appendPrompt)
	}
	if !strings.Contains(child.appendPrompt, "Do not call "+mcpAskUser) {
		t.Errorf("child agent should be told not to ask_user, got %q", child.appendPrompt)
	}
	if strings.Contains(child.appendPrompt, mcpCreateWorkspace) {
		t.Errorf("child agent must not be pointed at create_workspace, got %q", child.appendPrompt)
	}
	if !shapeHasTool(child.tools, mcpRenameBranch) {
		t.Errorf("child agent missing %s; tools = %v", mcpRenameBranch, child.tools)
	}
	if !shapeHasTool(child.tools, mcpSwitchMode) {
		t.Errorf("child agent missing %s; tools = %v", mcpSwitchMode, child.tools)
	}
	if !shapeHasTool(shape.tools, mcpAskUser) {
		t.Errorf("parent agent missing %s; tools = %v", mcpAskUser, shape.tools)
	}
	for _, tl := range []string{mcpAskUser, mcpOpenPR, mcpCreateWorkspace, mcpMergeChild, mcpReadChild, mcpArchiveChild, mcpMessageChild} {
		if shapeHasTool(child.tools, tl) {
			t.Errorf("child agent must not allow %s; tools = %v", tl, child.tools)
		}
	}
	for _, tl := range []string{mcpOpenPR, mcpCreateWorkspace, mcpMergeChild, mcpReadChild, mcpArchiveChild, mcpMessageChild} {
		if !shapeHasTool(shape.tools, tl) {
			t.Errorf("parent agent missing %s; tools = %v", tl, shape.tools)
		}
	}
	if !strings.Contains(shape.appendPrompt, mcpMessageChild) {
		t.Errorf("agent appendPrompt should name message_child, got %q", shape.appendPrompt)
	}
	for _, mode := range []string{domain.ChatModeAsk, domain.ChatModePlan, domain.ChatModeReview} {
		other := chatModeShape(mode, domain.Settings{}, false)
		if strings.Contains(other.appendPrompt, "beyond a very simple task") {
			t.Errorf("chatModeShape(%q) must not carry agent child-workspace guidance, got %q", mode, other.appendPrompt)
		}
		if shapeHasTool(other.tools, mcpMessageChild) {
			t.Errorf("chatModeShape(%q) must not allow %s; tools = %v", mode, mcpMessageChild, other.tools)
		}
		if !shapeHasTool(other.tools, mcpAskUser) {
			t.Errorf("chatModeShape(%q) parent missing %s; tools = %v", mode, mcpAskUser, other.tools)
		}
		nested := chatModeShape(mode, domain.Settings{}, true)
		if strings.Contains(nested.appendPrompt, "execute the assigned task here") {
			t.Errorf("chatModeShape(%q) child must not carry agent execute-locally guidance, got %q", mode, nested.appendPrompt)
		}
		if shapeHasTool(nested.tools, mcpAskUser) {
			t.Errorf("chatModeShape(%q) child must not allow %s; tools = %v", mode, mcpAskUser, nested.tools)
		}
	}
}

func TestApplyCommentHygiene(t *testing.T) {
	edit := AgentSpec{AllowedTools: []string{"Read", "Edit", "Write"}}
	applyCommentHygiene(&edit)
	if !strings.Contains(edit.AppendSystemPrompt, commentHygieneNeedle) {
		t.Errorf("write turn missing comment hygiene, got %q", edit.AppendSystemPrompt)
	}
	if !strings.Contains(edit.AppendSystemPrompt, "explains itself") ||
		!strings.Contains(edit.AppendSystemPrompt, "unless the user asked to remove") {
		t.Errorf("comment hygiene missing nuance (complex ok, user can strip), got %q", edit.AppendSystemPrompt)
	}
	once := edit.AppendSystemPrompt
	applyCommentHygiene(&edit)
	if edit.AppendSystemPrompt != once {
		t.Errorf("comment hygiene injected twice: %q", edit.AppendSystemPrompt)
	}

	ask := AgentSpec{AllowedTools: []string{"Read", "Grep"}}
	applyCommentHygiene(&ask)
	if ask.AppendSystemPrompt != "" {
		t.Errorf("read-only turn got comment hygiene: %q", ask.AppendSystemPrompt)
	}

	empty := AgentSpec{}
	applyCommentHygiene(&empty)
	if empty.AppendSystemPrompt != "" {
		t.Errorf("empty allowlist got comment hygiene: %q", empty.AppendSystemPrompt)
	}
}

func shapeHasTool(tools []string, want string) bool {
	for _, tl := range tools {
		if tl == want {
			return true
		}
	}
	return false
}
