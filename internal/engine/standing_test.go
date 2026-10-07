package engine

import "testing"

func TestStandingInUserPrompt(t *testing.T) {
	tests := []struct {
		name    string
		spec    AgentSpec
		resumed bool
		want    bool
	}{
		{name: "fresh turn", spec: AgentSpec{}, resumed: false, want: true},
		{name: "ordinary resume", spec: AgentSpec{ResumeSession: "s"}, resumed: true, want: false},
		{name: "failed resume fallback", spec: AgentSpec{ResumeSession: "s"}, resumed: false, want: true},
		{
			name:    "mode remount",
			spec:    AgentSpec{ResumeSession: "s", RefreshStandingInstructions: true},
			resumed: true,
			want:    true,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := standingInUserPrompt(tc.spec, tc.resumed)
			if got != tc.want {
				t.Errorf("standingInUserPrompt(%s, resumed=%v) = %v, want %v",
					tc.name, tc.resumed, got, tc.want)
			}
		})
	}
}
