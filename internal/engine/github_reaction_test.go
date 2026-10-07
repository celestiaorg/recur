package engine

import (
	"context"
	"testing"

	"github.com/google/go-cmp/cmp"

	"recur/internal/domain"
)

func TestParseGitHubReactions(t *testing.T) {
	raw := `{
	  "data": {
	    "repository": {
	      "pullRequest": {
	        "id": "PR_1",
	        "reactionGroups": [
	          {"content":"HOORAY","viewerHasReacted":false,"reactors":{"totalCount":3}}
	        ],
	        "comments": {
	          "nodes": [
	            {"id":"IC_1","databaseId":1001,"reactionGroups":[
	              {"content":"THUMBS_UP","viewerHasReacted":false,"reactors":{"totalCount":2}},
	              {"content":"HEART","viewerHasReacted":true,"reactors":{"totalCount":1}}
	            ]}
	          ]
	        },
	        "reviews": {
	          "nodes": [
	            {"id":"PRR_1","databaseId":99,"reactionGroups":[
	              {"content":"ROCKET","viewerHasReacted":true,"reactors":{"totalCount":1}}
	            ]}
	          ]
	        },
	        "reviewThreads": {
	          "nodes": [
	            {"comments":{"nodes":[
	              {"id":"PRRC_1","databaseId":1,"reactionGroups":[
	                {"content":"EYES","viewerHasReacted":false,"reactors":{"totalCount":1}}
	              ]}
	            ]}}
	          ]
	        }
	      }
	    }
	  }
	}`
	hits, nodeID, err := parseGitHubReactions(raw)
	if err != nil {
		t.Fatalf("parseGitHubReactions() error = %v, want nil", err)
	}
	if nodeID != "PR_1" {
		t.Errorf("parseGitHubReactions() nodeID = %q, want PR_1", nodeID)
	}
	cs := []domain.PRComment{
		{ID: "gh-description"},
		{ID: "gh-comment-IC_1"},
		{ID: "gh-review-PRR_1"},
		{ID: "gh-review-action-99", Kind: "action"},
		{ID: "gh-inline-1"},
	}
	applyGitHubReactions(cs, hits)
	if cs[0].NodeID != "PR_1" || !domain.ReactionsEqual(cs[0].Reactions, []domain.PRReaction{{Content: "hooray", Count: 3}}) {
		t.Errorf("applyGitHubReactions description = %+v, want PR_1 hooray×3", cs[0])
	}
	wantComment := []domain.PRReaction{{Content: "+1", Count: 2}, {Content: "heart", Count: 1, Mine: true}}
	if cs[1].NodeID != "IC_1" || !domain.ReactionsEqual(cs[1].Reactions, wantComment) {
		t.Errorf("applyGitHubReactions comment = %+v, want IC_1 +1/heart", cs[1])
	}
	if cs[2].NodeID != "PRR_1" || !domain.ReactionsEqual(cs[2].Reactions, []domain.PRReaction{{Content: "rocket", Count: 1, Mine: true}}) {
		t.Errorf("applyGitHubReactions review = %+v, want PRR_1 rocket", cs[2])
	}
	if cs[3].NodeID != "PRR_1" || !domain.ReactionsEqual(cs[3].Reactions, []domain.PRReaction{{Content: "rocket", Count: 1, Mine: true}}) {
		t.Errorf("applyGitHubReactions review action = %+v, want PRR_1 rocket", cs[3])
	}
	if cs[4].NodeID != "PRRC_1" || !domain.ReactionsEqual(cs[4].Reactions, []domain.PRReaction{{Content: "eyes", Count: 1}}) {
		t.Errorf("applyGitHubReactions inline = %+v, want PRRC_1 eyes", cs[4])
	}
}

func TestReactionSubjectID(t *testing.T) {
	pr := domain.PR{NodeID: "PR_1"}
	if got := reactionSubjectID(domain.PRComment{ID: "gh-description"}, pr); got != "PR_1" {
		t.Errorf("reactionSubjectID(description) = %q, want PR_1", got)
	}
	if got := reactionSubjectID(domain.PRComment{ID: "gh-comment-IC_1", NodeID: "IC_1"}, pr); got != "IC_1" {
		t.Errorf("reactionSubjectID(node) = %q, want IC_1", got)
	}
	if got := reactionSubjectID(domain.PRComment{ID: "gh-review-PRR_9"}, pr); got != "PRR_9" {
		t.Errorf("reactionSubjectID(review graphql) = %q, want PRR_9", got)
	}
	if got := reactionSubjectID(domain.PRComment{ID: "gh-inline-1"}, pr); got != "" {
		t.Errorf("reactionSubjectID(inline rest) = %q, want empty", got)
	}
}

func TestTogglePRCommentReactionLocal(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	pr, err := st.AddPR(domain.PR{
		ID: "local-rxn", Source: domain.SourceLocal,
		Comments: []domain.PRComment{{ID: "c1", Author: "You", BodyMD: "hi"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, ok := eng.TogglePRCommentReaction(pr.ID, "c1", "+1")
	if !ok {
		t.Fatal("TogglePRCommentReaction(+1) = false, want true")
	}
	if diff := cmp.Diff([]domain.PRReaction{{Content: "+1", Count: 1, Mine: true}}, got.Comments[0].Reactions); diff != "" {
		t.Errorf("TogglePRCommentReaction(+1) mismatch (-want +got):\n%s", diff)
	}
	got, ok = eng.TogglePRCommentReaction(pr.ID, "c1", "+1")
	if !ok || len(got.Comments[0].Reactions) != 0 {
		t.Errorf("TogglePRCommentReaction(+1 again) = ok %t reactions %+v, want empty", ok, got.Comments[0].Reactions)
	}
}

func TestTogglePRCommentReactionGitHub(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	pr, err := st.AddPR(domain.PR{
		ID: "gh-rxn", Number: 12, Source: domain.SourceGitHub,
		ViewerLogin: "avery", NodeID: "PR_1",
		Comments: []domain.PRComment{
			{ID: "gh-comment-IC_1", NodeID: "IC_1", AuthorLogin: "sam", BodyMD: "looks good"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	var gotSubject, gotContent string
	var gotAdd bool
	prev := setGitHubReactionFn
	t.Cleanup(func() { setGitHubReactionFn = prev })
	setGitHubReactionFn = func(_ context.Context, _ string, subject, content string, add bool) error {
		gotSubject, gotContent, gotAdd = subject, content, add
		return nil
	}
	updated, ok := eng.TogglePRCommentReaction(pr.ID, "gh-comment-IC_1", "heart")
	if !ok {
		t.Fatal("TogglePRCommentReaction(heart) = false, want true")
	}
	if gotSubject != "IC_1" || gotContent != "heart" || !gotAdd {
		t.Errorf("setGitHubReactionFn = %q %q add=%t, want IC_1 heart true", gotSubject, gotContent, gotAdd)
	}
	if !domain.ReactionMine(updated.Comments[0].Reactions, "heart") {
		t.Errorf("TogglePRCommentReaction comments = %+v, want heart mine", updated.Comments[0].Reactions)
	}

	setGitHubReactionFn = func(_ context.Context, _ string, _ string, _ string, add bool) error {
		gotAdd = add
		return nil
	}
	updated, ok = eng.TogglePRCommentReaction(pr.ID, "gh-comment-IC_1", "heart")
	if !ok || gotAdd || domain.ReactionMine(updated.Comments[0].Reactions, "heart") {
		t.Errorf("TogglePRCommentReaction(remove) = ok %t add %t reactions %+v, want remove", ok, gotAdd, updated.Comments[0].Reactions)
	}
}

func TestRefreshPRConversationKeepsRecentReaction(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "hold-rxn", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-31", WorkspaceID: ws.ID, Number: 31, Source: domain.SourceGitHub,
		ViewerLogin: "avery",
		Comments: []domain.PRComment{{
			ID: "gh-comment-IC_1", BodyMD: "looks good",
			Reactions: []domain.PRReaction{{Content: "heart", Count: 1, Mine: true}},
		}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	eng.rememberCommentReactions(pr.ID, "gh-comment-IC_1", []domain.PRReaction{{Content: "heart", Count: 1, Mine: true}})
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			State: "OPEN",
			Comments: []domain.PRComment{{
				ID: "gh-comment-IC_1", BodyMD: "looks good",
			}},
		}, nil
	}
	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if len(got.Comments) != 1 || !domain.ReactionMine(got.Comments[0].Reactions, "heart") {
		t.Errorf("RefreshPRConversation() comments = %+v, want gh-comment-IC_1 still heart", got.Comments)
	}
}
