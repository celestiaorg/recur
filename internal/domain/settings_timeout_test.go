package domain

import (
	"testing"
	"time"
)

func TestEffectiveAgentTurnTimeoutMinutes(t *testing.T) {
	tests := []struct {
		in   int
		want int
	}{
		{0, DefaultAgentTurnTimeoutMinutes},
		{-1, DefaultAgentTurnTimeoutMinutes},
		{1, MinAgentTurnTimeoutMinutes},
		{5, 5},
		{240, 240},
		{480, 480},
		{MaxAgentTurnTimeoutMinutes, MaxAgentTurnTimeoutMinutes},
		{MaxAgentTurnTimeoutMinutes + 1, MaxAgentTurnTimeoutMinutes},
	}
	for _, tc := range tests {
		if got := EffectiveAgentTurnTimeoutMinutes(tc.in); got != tc.want {
			t.Errorf("EffectiveAgentTurnTimeoutMinutes(%d) = %d, want %d", tc.in, got, tc.want)
		}
	}
}

func TestSettingsAgentTurnTimeout(t *testing.T) {
	if got := (Settings{}).AgentTurnTimeout(); got != 4*time.Hour {
		t.Errorf("zero Settings AgentTurnTimeout() = %v, want 4h", got)
	}
	if got := (Settings{AgentTurnTimeoutMinutes: 90}).AgentTurnTimeout(); got != 90*time.Minute {
		t.Errorf("90-minute Settings AgentTurnTimeout() = %v, want 90m", got)
	}
}

func TestEffectiveMaxLoopsAgents(t *testing.T) {
	if got := EffectiveMaxLoops(0); got != 0 {
		t.Errorf("EffectiveMaxLoops(0) = %d, want 0 (unlimited)", got)
	}
	if got := EffectiveMaxAgents(0); got != 0 {
		t.Errorf("EffectiveMaxAgents(0) = %d, want 0 (unlimited)", got)
	}
	if got := EffectiveMaxLoops(-1); got != DefaultMaxLoops {
		t.Errorf("EffectiveMaxLoops(-1) = %d, want default %d", got, DefaultMaxLoops)
	}
	if got := EffectiveMaxLoops(2); got != 2 {
		t.Errorf("EffectiveMaxLoops(2) = %d, want 2", got)
	}
	if got := EffectiveMaxAgents(128); got != 128 {
		t.Errorf("EffectiveMaxAgents(128) = %d, want 128 (no upper clamp)", got)
	}
}
