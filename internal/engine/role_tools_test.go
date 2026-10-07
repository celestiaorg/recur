package engine

import (
	"testing"

	"recur/internal/bundles"
	"recur/internal/domain"
)

func TestModeToolsIncludeSkillMCP(t *testing.T) {
	for _, mode := range []string{
		bundles.RoleModeAsk, bundles.RoleModePlan, bundles.RoleModeAgent,
		bundles.RoleModeReview, bundles.RoleModeLoop, "",
	} {
		got := ModeTools(mode)
		for _, want := range []string{mcpListSkills, mcpActivateSkill} {
			if !containsStr(got, want) {
				t.Errorf("ModeTools(%q) missing %q; tools=%v", mode, want, got)
			}
		}
	}
}

func TestRoleAllowlistAskPlusExtras(t *testing.T) {
	got := RoleAllowlist(bundles.Role{
		Mode:  bundles.RoleModeAsk,
		Tools: []string{"attach_evidence", "playwright", "report_result"},
	})
	for _, want := range []string{"Read", mcpAttachEvidence, mcpPlaywrightAll, mcpReportResult, mcpDone, mcpListSkills, mcpActivateSkill} {
		if !containsStr(got, want) {
			t.Errorf("RoleAllowlist(ask+qa extras) missing %q; tools=%v", want, got)
		}
	}
	if containsStr(got, "Edit") {
		t.Errorf("Ask role must not get Edit; tools=%v", got)
	}
}

func TestRoleAllowlistDropsDisabled(t *testing.T) {
	got := RoleAllowlist(bundles.Role{
		Mode:          bundles.RoleModeAgent,
		DisabledTools: []string{"WebSearch"},
	})
	if containsStr(got, "WebSearch") {
		t.Errorf("disabled WebSearch still present: %v", got)
	}
}

func TestMaestroToolsDropRetiredHands(t *testing.T) {
	for _, banned := range []string{mcpRerunReview, mcpResumeLoop, mcpExtendIterations} {
		if containsStr(maestroTools, banned) {
			t.Errorf("conductor allowlist still has retired %q", banned)
		}
	}
	for _, want := range []string{mcpExtendRounds, mcpReenterPhase, mcpRunPhase, mcpDispatchSpecialist, mcpRenameSession, mcpRenameWorkspace, mcpListSkills, mcpActivateSkill, mcpMessageChild} {
		if !containsStr(maestroTools, want) {
			t.Errorf("conductor allowlist missing %q", want)
		}
	}
}

func TestModelForSlotDefaultsFromMode(t *testing.T) {
	set := domain.Settings{
		ModelAgent:  "openai/gpt-4.1",
		ModelReview: "openai/o3",
		ModelLoop:   "openai/gpt-5",
	}
	got, _ := modelForSlot(set, roleSpec{mode: bundles.RoleModeAgent}, "fallback", nil)
	if want := set.AgentModel().String(); got != want {
		t.Errorf("modelForSlot(mode=agent) = %q, want %q", got, want)
	}
	got, _ = modelForSlot(set, roleSpec{mode: bundles.RoleModeReview}, "fallback", nil)
	if want := set.ReviewModel().String(); got != want {
		t.Errorf("modelForSlot(mode=review) = %q, want %q", got, want)
	}
	got, _ = modelForSlot(set, roleSpec{modelSlot: bundles.RoleModeLoop, mode: bundles.RoleModeAgent}, "fallback", nil)
	if want := set.LoopModel().String(); got != want {
		t.Errorf("modelForSlot(slot=loop, mode=agent) = %q, want %q", got, want)
	}
}
