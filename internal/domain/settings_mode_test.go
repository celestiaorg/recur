package domain

import "testing"

func TestSkillEnabled(t *testing.T) {
	var empty Settings
	if !empty.SkillEnabled("agent-onboarding") {
		t.Fatal("nil DisabledSkills should enable all")
	}
	set := Settings{DisabledSkills: []string{"agent-onboarding", "frontend-design"}}
	if set.SkillEnabled("agent-onboarding") {
		t.Fatal("disabled skill should be off")
	}
	if !set.SkillEnabled("create-skill") {
		t.Fatal("unlisted skill should stay on")
	}
}

func TestSimplifyEnabled(t *testing.T) {
	var empty Settings
	if !empty.SimplifyEnabled() {
		t.Fatal("empty settings should default Simplify on")
	}
	off := false
	if (Settings{Simplify: &off}).SimplifyEnabled() {
		t.Fatal("explicit false should be off")
	}
	on := true
	if !(Settings{Simplify: &on}).SimplifyEnabled() {
		t.Fatal("explicit true should be on")
	}
}

func TestEffectiveSkillEnabled(t *testing.T) {
	global := []string{"noisy", "go-code-review"}
	// Inherit global off.
	if EffectiveSkillEnabled("noisy", global, nil, nil) {
		t.Fatal("should inherit global disable")
	}
	// Project force-on overrides global off.
	if !EffectiveSkillEnabled("noisy", global, nil, []string{"noisy"}) {
		t.Fatal("project enable should win over global disable")
	}
	// Project force-off overrides global on.
	if EffectiveSkillEnabled("create-skill", global, []string{"create-skill"}, nil) {
		t.Fatal("project disable should win")
	}
	// Project disable wins if listed in both.
	if EffectiveSkillEnabled("noisy", global, []string{"noisy"}, []string{"noisy"}) {
		t.Fatal("project disable should beat project enable")
	}
}

func TestModelForChatMode(t *testing.T) {
	set := Settings{
		ModelAsk:    "claude-code/haiku",
		ModelPlan:   "claude-code/opus",
		ModelAgent:  "claude-code/sonnet",
		ModelReview: "claude-code/fable",
		ModelLoop:   "claude-code/opus",
		FlagsAsk:    map[string]string{"effort": "low"},
		FlagsPlan:   map[string]string{"effort": "high"},
		FlagsAgent:  map[string]string{"effort": "medium"},
		FlagsReview: map[string]string{"effort": "xhigh"},
		FlagsLoop:   map[string]string{"effort": "high"},
	}

	cases := []struct {
		mode      string
		wantModel string
		wantFlag  string
	}{
		{ChatModeAsk, "claude-code/haiku", "low"},
		{ChatModePlan, "claude-code/opus", "high"},
		{ChatModeAgent, "claude-code/sonnet", "medium"},
		{"", "claude-code/sonnet", "medium"}, // unknown/empty → agent
		{ChatModeReview, "claude-code/fable", "xhigh"},
		{ChatModeLoop, "claude-code/opus", "high"},
	}
	for _, tc := range cases {
		model, flags := set.ModelForChatMode(tc.mode)
		if model != tc.wantModel {
			t.Errorf("mode %q model=%q, want %q", tc.mode, model, tc.wantModel)
		}
		if flags["effort"] != tc.wantFlag {
			t.Errorf("mode %q effort=%q, want %q", tc.mode, flags["effort"], tc.wantFlag)
		}
	}
}

func TestChildWorkspaceModel(t *testing.T) {
	cases := []struct {
		in, want string
	}{
		{in: "", want: ChildWorkspaceModelParent},
		{in: ChildWorkspaceModelParent, want: ChildWorkspaceModelParent},
		{in: ChildWorkspaceModelMode, want: ChildWorkspaceModelMode},
		{in: "junk", want: ChildWorkspaceModelParent},
	}
	for _, tc := range cases {
		got := (Settings{ChildWorkspaceModel: tc.in}).EffectiveChildWorkspaceModel()
		if got != tc.want {
			t.Errorf("EffectiveChildWorkspaceModel(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestModelForChatModeReviewFallsBackToLoop(t *testing.T) {
	set := Settings{
		ModelAsk:   "claude-code/haiku",
		ModelAgent: "claude-code/sonnet",
		ModelLoop:  "claude-code/opus",
	}
	model, _ := set.ModelForChatMode(ChatModeReview)
	if model != "claude-code/opus" {
		t.Fatalf("empty ModelReview should fall back to Loop, got %q", model)
	}
}

func TestLoopRoleAccessorsAliasModes(t *testing.T) {
	set := Settings{
		ModelAgent:  "cursor/composer-2.5",
		ModelReview: "claude-code/haiku",
		ModelLoop:   "claude-code/opus",
		FlagsAgent:  map[string]string{"fast": "true"},
		FlagsReview: map[string]string{"effort": "low"},
		FlagsLoop:   map[string]string{"effort": "high"},
	}
	if got := set.MaestroModel().String(); got != "claude-code/opus" {
		t.Errorf("MaestroModel=%q, want Loop", got)
	}
	if got := set.CoderModel().String(); got != "cursor/composer-2.5" {
		t.Errorf("CoderModel=%q, want Agent", got)
	}
	if got := set.ReviewerModel().String(); got != "claude-code/haiku" {
		t.Errorf("ReviewerModel=%q, want Review", got)
	}
	if set.MaestroFlags()["effort"] != "high" || set.CoderFlags()["fast"] != "true" || set.ReviewerFlags()["effort"] != "low" {
		t.Errorf("role flag aliases mismatch: maestro=%v coder=%v reviewer=%v",
			set.MaestroFlags(), set.CoderFlags(), set.ReviewerFlags())
	}
}
