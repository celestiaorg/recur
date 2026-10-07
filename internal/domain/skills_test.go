package domain

import (
	"reflect"
	"testing"
)

func TestSkillRoots(t *testing.T) {
	ws, user := SkillRoots()
	wantWS := []string{
		".cursor/skills",
		".agents/skills",
		".claude/skills",
		".codex/skills",
	}
	wantUser := []string{
		".cursor/skills",
		".agents/skills",
		".claude/skills",
		".codex/skills",
		".recur/recur-skills",
	}
	if !reflect.DeepEqual(ws, wantWS) {
		t.Fatalf("workspace=%v, want %v", ws, wantWS)
	}
	if !reflect.DeepEqual(user, wantUser) {
		t.Fatalf("user=%v, want %v", user, wantUser)
	}
}
