package domain

import "testing"

func TestEffectiveBranchNamePrefix(t *testing.T) {
	cases := []struct {
		name string
		set  Settings
		want string
	}{
		{name: "unset is recur", want: DefaultBranchNamePrefix},
		{name: "stored recur", set: Settings{BranchNamePrefix: PrefixString("recur")}, want: "recur"},
		{name: "stored custom", set: Settings{BranchNamePrefix: PrefixString("josh")}, want: "josh"},
		{name: "stored empty is none", set: Settings{BranchNamePrefix: PrefixString("")}, want: ""},
		{name: "stored trims", set: Settings{BranchNamePrefix: PrefixString("  team  ")}, want: "team"},
		{name: "legacy none", set: Settings{BranchPrefixMode: BranchPrefixNone}, want: ""},
		{name: "legacy custom", set: Settings{BranchPrefixMode: BranchPrefixCustom, BranchPrefixCustom: "josh"}, want: "josh"},
		{name: "legacy custom blank", set: Settings{BranchPrefixMode: BranchPrefixCustom}, want: "recur"},
		{name: "legacy github is recur without a stored string", set: Settings{BranchPrefixMode: BranchPrefixGitHub}, want: "recur"},
		{name: "string wins over legacy mode", set: Settings{BranchNamePrefix: PrefixString("alice"), BranchPrefixMode: BranchPrefixNone}, want: "alice"},
	}
	for _, tc := range cases {
		got := tc.set.EffectiveBranchNamePrefix()
		if got != tc.want {
			t.Errorf("EffectiveBranchNamePrefix(%s) = %q, want %q", tc.name, got, tc.want)
		}
	}
}
