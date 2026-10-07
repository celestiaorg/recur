package domain

import "testing"

func TestNormalizeMergeMethod(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want MergeMethod
	}{
		{"merge", MergeMethodMerge},
		{"squash", MergeMethodSquash},
		{"rebase", MergeMethodRebase},
		{"", MergeMethodMerge},
		{"bogus", MergeMethodMerge},
	} {
		if got := NormalizeMergeMethod(tc.in); got != tc.want {
			t.Errorf("NormalizeMergeMethod(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestEffectiveMergeMethod(t *testing.T) {
	for _, tc := range []struct {
		name string
		proj Project
		set  Settings
		want MergeMethod
	}{
		{"both empty → merge", Project{}, Settings{}, MergeMethodMerge},
		{"global set", Project{}, Settings{DefaultMergeMethod: MergeMethodSquash}, MergeMethodSquash},
		{"project overrides global", Project{MergeMethod: MergeMethodRebase}, Settings{DefaultMergeMethod: MergeMethodSquash}, MergeMethodRebase},
		{"project empty inherits global", Project{}, Settings{DefaultMergeMethod: MergeMethodRebase}, MergeMethodRebase},
		{"invalid project falls back to global", Project{MergeMethod: "bad"}, Settings{DefaultMergeMethod: MergeMethodSquash}, MergeMethodSquash},
		{"invalid both falls back to merge", Project{MergeMethod: "bad"}, Settings{DefaultMergeMethod: "bad"}, MergeMethodMerge},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := EffectiveMergeMethod(tc.proj, tc.set); got != tc.want {
				t.Errorf("EffectiveMergeMethod = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestMergeMethodPermitted(t *testing.T) {
	if !MergeMethodPermitted(nil, MergeMethodRebase) {
		t.Error("empty allowed should permit every method")
	}
	if !MergeMethodPermitted([]string{"squash"}, MergeMethodSquash) {
		t.Error("squash should be permitted")
	}
	if MergeMethodPermitted([]string{"squash"}, MergeMethodMerge) {
		t.Error("merge should be refused when only squash is allowed")
	}
}

func TestMergeMethodLabel(t *testing.T) {
	if got := MergeMethodLabel(MergeMethodMerge); got != "Merge" {
		t.Errorf("label(merge) = %q", got)
	}
	if got := MergeMethodLabel(MergeMethodSquash); got != "Squash and merge" {
		t.Errorf("label(squash) = %q", got)
	}
	if got := MergeMethodLabel(MergeMethodRebase); got != "Rebase and merge" {
		t.Errorf("label(rebase) = %q", got)
	}
}

func TestNormalizeUpdateBaseMethod(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want UpdateBaseMethod
	}{
		{"merge", UpdateBaseMerge},
		{"rebase", UpdateBaseRebase},
		{"", UpdateBaseMerge},
		{"squash", UpdateBaseMerge}, // squash is not valid for update-base
		{"bogus", UpdateBaseMerge},
	} {
		if got := NormalizeUpdateBaseMethod(tc.in); got != tc.want {
			t.Errorf("NormalizeUpdateBaseMethod(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestEffectiveUpdateBaseMethod(t *testing.T) {
	for _, tc := range []struct {
		name string
		proj Project
		set  Settings
		want UpdateBaseMethod
	}{
		{"both empty → merge", Project{}, Settings{}, UpdateBaseMerge},
		{"global set", Project{}, Settings{DefaultUpdateBaseMethod: UpdateBaseRebase}, UpdateBaseRebase},
		{"project overrides global", Project{UpdateBaseMethod: UpdateBaseMerge}, Settings{DefaultUpdateBaseMethod: UpdateBaseRebase}, UpdateBaseMerge},
		{"project empty inherits global", Project{}, Settings{DefaultUpdateBaseMethod: UpdateBaseRebase}, UpdateBaseRebase},
		{"invalid project falls back to global", Project{UpdateBaseMethod: "bad"}, Settings{DefaultUpdateBaseMethod: UpdateBaseRebase}, UpdateBaseRebase},
		{"invalid both falls back to merge", Project{UpdateBaseMethod: "bad"}, Settings{DefaultUpdateBaseMethod: "bad"}, UpdateBaseMerge},
		{"independent of MergeMethod", Project{MergeMethod: MergeMethodSquash}, Settings{DefaultMergeMethod: MergeMethodSquash}, UpdateBaseMerge},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := EffectiveUpdateBaseMethod(tc.proj, tc.set); got != tc.want {
				t.Errorf("EffectiveUpdateBaseMethod = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestUpdateBaseMethodLabel(t *testing.T) {
	if got := UpdateBaseMethodLabel(UpdateBaseMerge); got != "merge" {
		t.Errorf("label(merge) = %q", got)
	}
	if got := UpdateBaseMethodLabel(UpdateBaseRebase); got != "rebase" {
		t.Errorf("label(rebase) = %q", got)
	}
}

func TestNormalizeSquashCommitMessage(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want SquashCommitMessage
	}{
		{"title", SquashCommitTitle},
		{"title-and-commits", SquashCommitTitleAndCommits},
		{"title-and-description", SquashCommitTitleAndDescription},
		{"", SquashCommitTitleAndCommits},
		{"bogus", SquashCommitTitleAndCommits},
	} {
		if got := NormalizeSquashCommitMessage(tc.in); got != tc.want {
			t.Errorf("NormalizeSquashCommitMessage(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestEffectiveSquashCommitMessage(t *testing.T) {
	for _, tc := range []struct {
		name string
		proj Project
		set  Settings
		want SquashCommitMessage
	}{
		{"both empty → title-and-commits", Project{}, Settings{}, SquashCommitTitleAndCommits},
		{"global set", Project{}, Settings{DefaultSquashCommitMessage: SquashCommitTitle}, SquashCommitTitle},
		{"project overrides global", Project{SquashCommitMessage: SquashCommitTitleAndDescription}, Settings{DefaultSquashCommitMessage: SquashCommitTitle}, SquashCommitTitleAndDescription},
		{"project empty inherits global", Project{}, Settings{DefaultSquashCommitMessage: SquashCommitTitleAndDescription}, SquashCommitTitleAndDescription},
		{"invalid project falls back to global", Project{SquashCommitMessage: "bad"}, Settings{DefaultSquashCommitMessage: SquashCommitTitle}, SquashCommitTitle},
		{"invalid both falls back to title-and-commits", Project{SquashCommitMessage: "bad"}, Settings{DefaultSquashCommitMessage: "bad"}, SquashCommitTitleAndCommits},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := EffectiveSquashCommitMessage(tc.proj, tc.set); got != tc.want {
				t.Errorf("EffectiveSquashCommitMessage = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestNormalizeMergeCommitMessage(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want MergeCommitMessage
	}{
		{"title", MergeCommitTitle},
		{"title-and-description", MergeCommitTitleAndDescription},
		{"", MergeCommitTitle},
		{"title-and-commits", MergeCommitTitle}, // squash-only value is invalid here
		{"bogus", MergeCommitTitle},
	} {
		if got := NormalizeMergeCommitMessage(tc.in); got != tc.want {
			t.Errorf("NormalizeMergeCommitMessage(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestEffectiveMergeCommitMessage(t *testing.T) {
	for _, tc := range []struct {
		name string
		proj Project
		set  Settings
		want MergeCommitMessage
	}{
		{"both empty → title", Project{}, Settings{}, MergeCommitTitle},
		{"global set", Project{}, Settings{DefaultMergeCommitMessage: MergeCommitTitleAndDescription}, MergeCommitTitleAndDescription},
		{"project overrides global", Project{MergeCommitMessage: MergeCommitTitle}, Settings{DefaultMergeCommitMessage: MergeCommitTitleAndDescription}, MergeCommitTitle},
		{"project empty inherits global", Project{}, Settings{DefaultMergeCommitMessage: MergeCommitTitleAndDescription}, MergeCommitTitleAndDescription},
		{"invalid project falls back to global", Project{MergeCommitMessage: "bad"}, Settings{DefaultMergeCommitMessage: MergeCommitTitleAndDescription}, MergeCommitTitleAndDescription},
		{"invalid both falls back to title", Project{MergeCommitMessage: "bad"}, Settings{DefaultMergeCommitMessage: "bad"}, MergeCommitTitle},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := EffectiveMergeCommitMessage(tc.proj, tc.set); got != tc.want {
				t.Errorf("EffectiveMergeCommitMessage = %q, want %q", got, tc.want)
			}
		})
	}
}
