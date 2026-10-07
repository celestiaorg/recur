package engine

import (
	"cmp"
	"slices"
	"strings"

	"recur/internal/domain"
)

func reviewerKey(r domain.PRReviewer) string {
	return participantKey(domain.PRParticipant{
		Login: r.Login,
		Name:  r.Name,
		Code:  r.Code,
	})
}

func actorReviewer(a ghListedActor, state string) domain.PRReviewer {
	p := actorParticipant(a)
	return domain.PRReviewer{
		Login:     p.Login,
		Name:      p.Name,
		AvatarURL: p.AvatarURL,
		AvatarCSS: p.AvatarCSS,
		Code:      p.Code,
		State:     state,
	}
}

func commentReviewer(c domain.PRComment, state string) domain.PRReviewer {
	p := commentParticipant(c)
	return domain.PRReviewer{
		Login:     p.Login,
		Name:      p.Name,
		AvatarURL: p.AvatarURL,
		AvatarCSS: p.AvatarCSS,
		Code:      p.Code,
		State:     state,
	}
}

func upsertReviewer(out []domain.PRReviewer, r domain.PRReviewer) []domain.PRReviewer {
	r.State = strings.TrimSpace(r.State)
	key := reviewerKey(r)
	if key == "" || r.State == "" {
		return out
	}
	for i, have := range out {
		if reviewerKey(have) != key {
			continue
		}
		if r.AvatarURL != "" {
			out[i].AvatarURL = r.AvatarURL
		}
		if r.AvatarCSS != "" {
			out[i].AvatarCSS = r.AvatarCSS
		}
		if r.Name != "" && out[i].Name == "" {
			out[i].Name = r.Name
		}
		out[i].State = r.State
		return out
	}
	return append(out, r)
}

func requestReviewer(out []domain.PRReviewer, a ghListedActor) []domain.PRReviewer {
	p := actorParticipant(a)
	key := participantKey(p)
	if key == "" {
		return out
	}
	for _, have := range out {
		if reviewerKey(have) != key {
			continue
		}
		if have.State == domain.ReviewStateApproved || have.State == domain.ReviewStateChangesRequested {
			return out
		}
		break
	}
	return upsertReviewer(out, actorReviewer(a, domain.ReviewStatePending))
}

func reviewStateRank(state string) int {
	switch state {
	case domain.ReviewStateChangesRequested:
		return 0
	case domain.ReviewStatePending:
		return 1
	case domain.ReviewStateApproved:
		return 2
	case domain.ReviewStateCommented:
		return 3
	default:
		return 9
	}
}

func normalizeGHReviewState(state string) string {
	switch strings.ToUpper(strings.TrimSpace(state)) {
	case "APPROVED":
		return domain.ReviewStateApproved
	case "CHANGES_REQUESTED":
		return domain.ReviewStateChangesRequested
	case "COMMENTED":
		return domain.ReviewStateCommented
	default:
		return ""
	}
}

func reviewStateFromVerb(verb string) string {
	switch strings.ToLower(strings.TrimSpace(verb)) {
	case "approved":
		return domain.ReviewStateApproved
	case "requested changes", "changes requested":
		return domain.ReviewStateChangesRequested
	case "commented", "reviewed":
		return domain.ReviewStateCommented
	default:
		return ""
	}
}

func commentReviewState(c domain.PRComment) string {
	if c.Kind == "hidden" || c.Kind == "description" {
		return ""
	}
	st := reviewStateFromVerb(c.Verb)
	if st == domain.ReviewStateApproved || st == domain.ReviewStateChangesRequested {
		return st
	}
	if c.IsReviewCard() {
		if st == "" {
			return domain.ReviewStateCommented
		}
		return st
	}
	return ""
}

func sortReviewers(out []domain.PRReviewer) []domain.PRReviewer {
	slices.SortStableFunc(out, func(a, b domain.PRReviewer) int {
		if n := cmp.Compare(reviewStateRank(a.State), reviewStateRank(b.State)); n != 0 {
			return n
		}
		return cmp.Compare(strings.ToLower(reviewerKey(a)), strings.ToLower(reviewerKey(b)))
	})
	return out
}

func listedReviewers(row ghListedPR) []domain.PRReviewer {
	var out []domain.PRReviewer
	for _, r := range row.LatestReviews {
		st := normalizeGHReviewState(r.State)
		if st == "" {
			continue
		}
		out = upsertReviewer(out, actorReviewer(r.Author, st))
	}
	for _, a := range row.ReviewRequests {
		out = requestReviewer(out, a)
	}
	return sortReviewers(out)
}

func snapshotReviewers(snap ghPRSnapshot, comments []domain.PRComment) []domain.PRReviewer {
	out := commentsReviewers(comments)
	for _, a := range snap.ReviewRequests {
		out = requestReviewer(out, a)
	}
	return sortReviewers(out)
}

func commentsReviewers(comments []domain.PRComment) []domain.PRReviewer {
	var out []domain.PRReviewer
	for _, c := range comments {
		st := commentReviewState(c)
		if st == "" {
			continue
		}
		out = upsertReviewer(out, commentReviewer(c, st))
	}
	return sortReviewers(out)
}
