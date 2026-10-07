package engine

import (
	"testing"

	"recur/internal/domain"
)

// TestFeedItemKindStampedAtSource covers S2's typed-feed change: the engine stamps
// FeedItem.Kind at the source (tool / outcome / note / deviation) so the frontend
// reads a field instead of regex-classifying rendered HTML.
func TestFeedItemKindStampedAtSource(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	const workspaceID = "LOOP-FEEDKIND"

	// outcome — an engine-authored milestone line (the emitWorkspaceEvent path)
	eng.emitWorkspaceEvent(workspaceID, "MA", "Maestro", "Converged to 0 findings.")
	// tool — a tool row carrying a ToolUseID
	eng.emitWorkspaceEventItem(workspaceID, domain.FeedItem{
		AgentCode: "CO", Name: "Coder", Text: "Read: <code>a.go</code>", ToolUseID: "tu-1",
	})
	// note — agent narration (stamped by the caller)
	eng.emitWorkspaceEventItem(workspaceID, domain.FeedItem{
		AgentCode: "CO", Name: "Coder", Text: "Looking at the code.", Kind: domain.FeedKindNote,
	})
	// deviation — a logged departure
	eng.emitDeviation(workspaceID, "Departed from the default", "the default", "a good reason")

	byText := map[string]string{} // text -> kind
	for _, it := range st.Feed(workspaceID) {
		byText[it.Text] = it.Kind
	}

	want := map[string]string{
		"Converged to 0 findings.":  domain.FeedKindOutcome,
		"Read: <code>a.go</code>":   domain.FeedKindTool,
		"Looking at the code.":      domain.FeedKindNote,
		"Departed from the default": domain.FeedKindDeviation,
	}
	for text, kind := range want {
		if got := byText[text]; got != kind {
			t.Errorf("feed item %q: kind = %q, want %q", text, got, kind)
		}
	}

	// A tool row's kind must survive the tool_result patch (which rewrites the row).
	if got := st.PatchFeedByToolUseID(workspaceID, "tu-1", func(it *domain.FeedItem) {
		it.ToolResult = "package a"
	}); got == nil || got.Kind != domain.FeedKindTool {
		t.Errorf("tool row kind lost after tool_result patch: %+v", got)
	}
}
