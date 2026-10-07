package engine

import (
	"testing"

	"recur/internal/domain"
)

func TestListedReviewersPendingAndApproved(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		ReviewRequests: []ghListedActor{{Login: "avery"}, {Login: "pat"}},
		LatestReviews: []ghListedReview{
			{Author: ghListedActor{Login: "sam"}, State: "APPROVED"},
			{Author: ghListedActor{Login: "pat"}, State: "COMMENTED"},
			{Author: ghListedActor{Login: "lee"}, State: "CHANGES_REQUESTED"},
			{Author: ghListedActor{Login: "ghost"}, State: "DISMISSED"},
		},
	}
	got := listedReviewers(row)
	want := []string{
		"lee:" + domain.ReviewStateChangesRequested,
		"avery:" + domain.ReviewStatePending,
		"pat:" + domain.ReviewStatePending,
		"sam:" + domain.ReviewStateApproved,
	}
	if len(got) != len(want) {
		t.Fatalf("listedReviewers = %+v, want %v", got, want)
	}
	for i, row := range want {
		gotRow := got[i].Login + ":" + got[i].State
		if gotRow != row {
			t.Errorf("listedReviewers[%d] = %s, want %s", i, gotRow, row)
		}
	}
}

func TestListedReviewersKeepsApprovalOverRequest(t *testing.T) {
	t.Parallel()
	row := ghListedPR{
		ReviewRequests: []ghListedActor{{Login: "sam"}},
		LatestReviews:  []ghListedReview{{Author: ghListedActor{Login: "sam"}, State: "APPROVED"}},
	}
	got := listedReviewers(row)
	if len(got) != 1 || got[0].Login != "sam" || got[0].State != domain.ReviewStateApproved {
		t.Fatalf("listedReviewers = %+v, want sam approved", got)
	}
}

func TestCommentsReviewersLatestWins(t *testing.T) {
	t.Parallel()
	got := commentsReviewers([]domain.PRComment{
		{ID: "gh-review-1", AuthorLogin: "lee", Verb: "requested changes"},
		{ID: "gh-review-2", AuthorLogin: "lee", Verb: "approved"},
		{ID: "c-issue", AuthorLogin: "sam", Verb: "commented"},
		{ID: "c-local", Author: "You", Verb: "approved"},
	})
	if len(got) != 2 {
		t.Fatalf("commentsReviewers = %+v, want lee + You", got)
	}
	if got[0].Login != "lee" || got[0].State != domain.ReviewStateApproved {
		t.Errorf("got[0] = %+v, want lee approved", got[0])
	}
	if got[1].Name != "You" || got[1].State != domain.ReviewStateApproved {
		t.Errorf("got[1] = %+v, want You approved", got[1])
	}
}

func TestSnapshotReviewersPendingWhenOnlyCommented(t *testing.T) {
	t.Parallel()
	snap := ghPRSnapshot{ReviewRequests: []ghListedActor{{Login: "avery"}}}
	got := snapshotReviewers(snap, []domain.PRComment{
		{ID: "gh-review-9", AuthorLogin: "avery", Verb: "commented"},
	})
	if len(got) != 1 || got[0].Login != "avery" || got[0].State != domain.ReviewStatePending {
		t.Fatalf("snapshotReviewers = %+v, want avery pending", got)
	}
}
