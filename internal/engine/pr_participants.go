package engine

import (
	"strings"

	"recur/internal/domain"
)

func participantKey(p domain.PRParticipant) string {
	if l := strings.ToLower(strings.TrimSpace(p.Login)); l != "" {
		return "login:" + l
	}
	if n := strings.ToLower(strings.TrimSpace(p.Name)); n != "" {
		return "name:" + n
	}
	if c := strings.ToLower(strings.TrimSpace(p.Code)); c != "" {
		return "code:" + c
	}
	return ""
}

func appendParticipant(out []domain.PRParticipant, p domain.PRParticipant) []domain.PRParticipant {
	p.Login = strings.TrimSpace(p.Login)
	p.Name = strings.TrimSpace(p.Name)
	p.AvatarURL = strings.TrimSpace(p.AvatarURL)
	p.AvatarCSS = strings.TrimSpace(p.AvatarCSS)
	p.Code = strings.TrimSpace(p.Code)
	key := participantKey(p)
	if key == "" {
		return out
	}
	for _, have := range out {
		if participantKey(have) == key {
			return out
		}
	}
	return append(out, p)
}

func mergeParticipants(base, extra []domain.PRParticipant) []domain.PRParticipant {
	out := append([]domain.PRParticipant(nil), base...)
	for _, p := range extra {
		out = appendParticipant(out, p)
	}
	return out
}

func actorParticipant(a ghListedActor) domain.PRParticipant {
	login := listedActorLogin(a)
	return domain.PRParticipant{
		Login:     login,
		AvatarURL: ghAvatarURL(login, strings.TrimSpace(a.AvatarURL)),
	}
}

func commentParticipant(c domain.PRComment) domain.PRParticipant {
	login := strings.TrimSpace(c.AuthorLogin)
	return domain.PRParticipant{
		Login:     login,
		Name:      strings.TrimSpace(c.Author),
		AvatarURL: ghAvatarURL(login, strings.TrimSpace(c.AvatarURL)),
		AvatarCSS: c.AvatarCSS,
		Code:      strings.TrimSpace(c.Code),
	}
}

func commitParticipant(c domain.Commit) domain.PRParticipant {
	code := strings.TrimSpace(c.AgentCode)
	name := strings.TrimSpace(c.AuthorRole)
	author := strings.TrimSpace(c.Author)
	if name == "" {
		name = author
	}
	login := ""
	if code == "" && author != "" && !strings.Contains(author, " ") {
		login = author
	}
	css := ""
	if code != "" {
		css = commentAvatarCSS(code)
	}
	return domain.PRParticipant{
		Login:     login,
		Name:      name,
		AvatarURL: ghAvatarURL(login, strings.TrimSpace(c.AvatarURL)),
		AvatarCSS: css,
		Code:      code,
	}
}

func listedParticipants(row ghListedPR) []domain.PRParticipant {
	var out []domain.PRParticipant
	out = appendParticipant(out, actorParticipant(row.Author))
	for _, a := range row.Assignees {
		out = appendParticipant(out, actorParticipant(a))
	}
	for _, a := range row.ReviewRequests {
		out = appendParticipant(out, actorParticipant(a))
	}
	for _, r := range row.LatestReviews {
		out = appendParticipant(out, actorParticipant(r.Author))
	}
	return out
}

func snapshotParticipants(snap ghPRSnapshot, comments []domain.PRComment) []domain.PRParticipant {
	var out []domain.PRParticipant
	if login := strings.TrimSpace(snap.AuthorLogin); login != "" {
		url := ""
		if snap.authorAvatars != nil {
			url = snap.authorAvatars[strings.ToLower(login)]
		}
		out = appendParticipant(out, domain.PRParticipant{
			Login:     login,
			AvatarURL: ghAvatarURL(login, url),
		})
	}
	for _, a := range snap.Assignees {
		out = appendParticipant(out, actorParticipant(a))
	}
	for _, a := range snap.ReviewRequests {
		out = appendParticipant(out, actorParticipant(a))
	}
	for _, c := range comments {
		if c.Kind == "hidden" {
			continue
		}
		out = appendParticipant(out, commentParticipant(c))
	}
	return out
}

func participantsFromPR(pr domain.PR) []domain.PRParticipant {
	var out []domain.PRParticipant
	if login := strings.TrimSpace(pr.AuthorLogin); login != "" {
		out = appendParticipant(out, domain.PRParticipant{
			Login:     login,
			AvatarURL: ghAvatarURL(login, ""),
		})
	}
	out = mergeParticipants(out, pr.Participants)
	for _, c := range pr.Comments {
		if c.Kind == "hidden" {
			continue
		}
		out = appendParticipant(out, commentParticipant(c))
	}
	for _, c := range pr.Commits {
		out = appendParticipant(out, commitParticipant(c))
	}
	return out
}
