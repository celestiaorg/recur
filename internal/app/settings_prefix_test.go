package app

import (
	"testing"

	"recur/internal/domain"
)

func TestSetSettingsNormalizesBranchNamePrefix(t *testing.T) {
	svc, _ := newTestServices(t)

	set := svc.Settings.GetSettings()
	if set.BranchNamePrefix == nil || *set.BranchNamePrefix != domain.DefaultBranchNamePrefix {
		t.Fatalf("GetSettings() prefix = %v, want %q", set.BranchNamePrefix, domain.DefaultBranchNamePrefix)
	}

	set.BranchNamePrefix = domain.PrefixString("My Team")
	got := mustSetSettings(t, svc.Settings, set)
	if got.BranchNamePrefix == nil || *got.BranchNamePrefix != "my-team" {
		t.Fatalf("SetSettings(My Team) prefix = %v, want my-team", got.BranchNamePrefix)
	}

	got.BranchNamePrefix = domain.PrefixString("!!!")
	got = mustSetSettings(t, svc.Settings, got)
	if got.BranchNamePrefix == nil || *got.BranchNamePrefix != "" {
		t.Fatalf("SetSettings(!!!) prefix = %v, want empty (none)", got.BranchNamePrefix)
	}
	if got.BranchPrefixMode != domain.BranchPrefixNone {
		t.Fatalf("SetSettings(!!!) mode = %q, want none", got.BranchPrefixMode)
	}

	got.BranchNamePrefix = domain.PrefixString("")
	got = mustSetSettings(t, svc.Settings, got)
	if got.BranchNamePrefix == nil || *got.BranchNamePrefix != "" {
		t.Fatalf("SetSettings(\"\") prefix = %v, want empty", got.BranchNamePrefix)
	}
}

func TestSetSettingsGovernorCaps(t *testing.T) {
	svc, _ := newTestServices(t)

	set := svc.Settings.GetSettings()
	set.MaxLoops = 0
	set.MaxAgents = 128
	got := mustSetSettings(t, svc.Settings, set)
	if got.MaxLoops != 0 {
		t.Fatalf("SetSettings(MaxLoops=0) = %d, want 0 (unlimited)", got.MaxLoops)
	}
	if got.MaxAgents != 128 {
		t.Fatalf("SetSettings(MaxAgents=128) = %d, want 128 (no upper clamp)", got.MaxAgents)
	}

	got.MaxLoops = -1
	got.MaxAgents = 3
	got = mustSetSettings(t, svc.Settings, got)
	if got.MaxLoops != domain.DefaultMaxLoops {
		t.Fatalf("SetSettings(MaxLoops=-1) = %d, want default %d", got.MaxLoops, domain.DefaultMaxLoops)
	}
	if got.MaxAgents != 3 {
		t.Fatalf("SetSettings(MaxAgents=3) = %d, want 3", got.MaxAgents)
	}
}
