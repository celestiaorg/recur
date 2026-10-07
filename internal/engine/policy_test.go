package engine

import (
	"testing"

	"recur/internal/bundles"
	"recur/internal/domain"
)

func TestSyntheticPolicyFromAutonomy(t *testing.T) {
	p := syntheticPolicy("recur.policies.cautious", domain.AutonomyCautious)
	if p.MaxRounds != 2 || p.PlanApproval != "always" {
		t.Fatalf("cautious: %+v", p)
	}
	p = syntheticPolicy(bundles.DefaultPolicyID, domain.AutonomyBalanced)
	if p.MaxRounds != 4 || p.FindingsThreshold.Levels["medium"] != 0 {
		t.Fatalf("balanced: %+v", p)
	}
	lp := syntheticPRLoop()
	if lp.Entry != "planning" {
		t.Fatalf("entry: %s", lp.Entry)
	}
	if !skipPlanApproval(p, domain.Workspace{}) {
		t.Fatal("balanced with no steps should skip plan approval")
	}
}

func TestFindingsBarCounts(t *testing.T) {
	th := bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0, "medium": 0}}
	if bundles.FindingsBlock(th, map[string]int{"low": 2, "info": 1}) {
		t.Fatal("low/info must not block balanced")
	}
	if !bundles.FindingsBlock(th, map[string]int{"medium": 1}) {
		t.Fatal("medium must block")
	}
}

func TestCountableFindingsNamedLevelsOnly(t *testing.T) {
	th := bundles.FindingsThreshold{Levels: map[string]int{"critical": 0, "high": 0, "medium": 0}}
	n := countableFindings(th, map[string]int{"medium": 1, "low": 9})
	if n != 1 {
		t.Fatalf("countable=%d, want 1 (lows are not named)", n)
	}
	n = countableFindings(bundles.FindingsThreshold{AllZero: true}, map[string]int{"low": 2, "info": 1})
	if n != 3 {
		t.Fatalf("all-zero countable=%d, want 3", n)
	}
}
