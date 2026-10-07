package domain

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestNormalizeReactionContent(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want string
		ok   bool
	}{
		{"+1", "+1", true},
		{"THUMBS_UP", "+1", true},
		{"thumbs_up", "+1", true},
		{"-1", "-1", true},
		{"THUMBS_DOWN", "-1", true},
		{"LAUGH", "laugh", true},
		{"Heart", "heart", true},
		{"", "", false},
		{"wave", "", false},
	} {
		got, ok := NormalizeReactionContent(tc.in)
		if got != tc.want || ok != tc.ok {
			t.Errorf("NormalizeReactionContent(%q) = %q %t, want %q %t", tc.in, got, ok, tc.want, tc.ok)
		}
	}
}

func TestGraphQLReactionContent(t *testing.T) {
	got, ok := GraphQLReactionContent("+1")
	if !ok || got != "THUMBS_UP" {
		t.Errorf("GraphQLReactionContent(+1) = %q %t, want THUMBS_UP true", got, ok)
	}
	if _, ok := GraphQLReactionContent("wave"); ok {
		t.Error("GraphQLReactionContent(wave) = true, want false")
	}
}

func TestToggleReactions(t *testing.T) {
	start := []PRReaction{{Content: "heart", Count: 1, Mine: true}, {Content: "+1", Count: 2}}
	added := ToggleReactions(start, "rocket")
	if diff := cmp.Diff([]PRReaction{
		{Content: "+1", Count: 2},
		{Content: "heart", Count: 1, Mine: true},
		{Content: "rocket", Count: 1, Mine: true},
	}, added); diff != "" {
		t.Errorf("ToggleReactions(add rocket) mismatch (-want +got):\n%s", diff)
	}
	removed := ToggleReactions(start, "heart")
	if diff := cmp.Diff([]PRReaction{{Content: "+1", Count: 2}}, removed); diff != "" {
		t.Errorf("ToggleReactions(remove heart) mismatch (-want +got):\n%s", diff)
	}
	joined := ToggleReactions([]PRReaction{{Content: "+1", Count: 2}}, "+1")
	if diff := cmp.Diff([]PRReaction{{Content: "+1", Count: 3, Mine: true}}, joined); diff != "" {
		t.Errorf("ToggleReactions(join +1) mismatch (-want +got):\n%s", diff)
	}
	if got := ToggleReactions(start, "wave"); !ReactionsEqual(got, start) {
		t.Errorf("ToggleReactions(wave) = %+v, want unchanged %+v", got, start)
	}
}

func TestCanReactComment(t *testing.T) {
	gh := PR{Source: SourceGitHub, ViewerLogin: "avery"}
	anon := PR{Source: SourceGitHub}
	local := PR{Source: SourceLocal}
	comment := PRComment{ID: "gh-comment-1"}
	review := PRComment{ID: "gh-review-action-1", Kind: "action"}
	merge := PRComment{ID: "gh-event-merged", Kind: "action"}
	commit := PRComment{ID: "gh-event-c", Kind: "action", CommitSHA: "abc"}
	desc := PRComment{ID: "gh-description", Kind: "description"}
	if !gh.CanReactComment(comment) || !gh.CanReactComment(review) || !gh.CanReactComment(desc) {
		t.Error("CanReactComment(gh viewer) = false, want true for comment/review/description")
	}
	if gh.CanReactComment(merge) || gh.CanReactComment(commit) || gh.CanReactComment(PRComment{Kind: "hidden"}) {
		t.Error("CanReactComment(gh) = true, want false for merge/commit/hidden")
	}
	if anon.CanReactComment(comment) {
		t.Error("CanReactComment(anon) = true, want false")
	}
	if !local.CanReactComment(comment) || local.CanReactComment(merge) {
		t.Error("CanReactComment(local) mismatch, want comment true merge false")
	}
}
