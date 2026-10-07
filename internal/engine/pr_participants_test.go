package engine

import (
	"testing"

	"recur/internal/domain"
)

func TestListedParticipantsOrderAndDedup(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		Author:         ghListedActor{Login: "sam"},
		Assignees:      []ghListedActor{{Login: "pat"}},
		ReviewRequests: []ghListedActor{{Login: "avery"}, {Login: "sam"}},
		LatestReviews:  []ghListedReview{{Author: ghListedActor{Login: "avery"}}, {Author: ghListedActor{Login: "copilot-pull-request-reviewer"}}},
	}
	got := listedParticipants(row)
	wantLogins := []string{"sam", "pat", "avery", "copilot-pull-request-reviewer"}
	if len(got) != len(wantLogins) {
		t.Fatalf("listedParticipants = %+v, want %d people", got, len(wantLogins))
	}
	for i, login := range wantLogins {
		if got[i].Login != login {
			t.Errorf("listedParticipants[%d].Login = %q, want %q", i, got[i].Login, login)
		}
		if got[i].AvatarURL == "" {
			t.Errorf("listedParticipants[%d] missing avatar url", i)
		}
	}
}

func TestMergeParticipantsKeepsExistingOrder(t *testing.T) {
	t.Parallel()
	base := []domain.PRParticipant{{Login: "loopbot"}, {Name: "Coder", Code: "CO"}}
	extra := []domain.PRParticipant{{Login: "avery"}, {Login: "loopbot"}}
	got := mergeParticipants(base, extra)
	if len(got) != 3 || got[0].Login != "loopbot" || got[1].Code != "CO" || got[2].Login != "avery" {
		t.Errorf("mergeParticipants = %+v", got)
	}
}

func TestParticipantsFromPRAuthorFirst(t *testing.T) {
	t.Parallel()
	got := participantsFromPR(domain.PR{
		AuthorLogin: "loopbot",
		Comments: []domain.PRComment{
			{Kind: "description", Author: "loopbot", AuthorLogin: "loopbot", Code: "LO"},
			{Author: "Avery Rourke", AuthorLogin: "avery", Code: "AR"},
			{Kind: "hidden", Author: "ghost", AuthorLogin: "ghost"},
			{Author: "Coder", Code: "CO"},
		},
	})
	if len(got) != 3 || got[0].Login != "loopbot" || got[1].Login != "avery" || got[2].Code != "CO" {
		t.Errorf("participantsFromPR = %+v", got)
	}
}
