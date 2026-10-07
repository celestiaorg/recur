package simplify

import (
	"strings"
	"testing"
)

func TestNoticeIsEmbedded(t *testing.T) {
	if !strings.Contains(Notice, "MIT License") || !strings.Contains(Notice, "Dietrich Gebert") {
		t.Fatalf("embedded NOTICE missing MIT attribution, got %q", Notice)
	}
}

func TestAppliesToChat(t *testing.T) {
	if !AppliesToChat("agent") || !AppliesToChat("plan") || !AppliesToChat("") {
		t.Fatal("agent/plan/empty should apply")
	}
	if AppliesToChat("ask") || AppliesToChat("review") || AppliesToChat("loop") {
		t.Fatal("ask/review/loop should not apply")
	}
}

func TestInstructions(t *testing.T) {
	got := Instructions()
	for _, want := range []string{"SIMPLIFY MODE", "YAGNI", "When NOT to be lazy", "simplify:"} {
		if !strings.Contains(got, want) {
			t.Errorf("Instructions missing %q", want)
		}
	}
	if strings.Contains(got, "PONYTAIL") || strings.Contains(got, "/ponytail") {
		t.Fatal("rails must not mention ponytail")
	}
}
