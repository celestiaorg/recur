package engine

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"html"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
)

// getGitHubPRSnapshot fetches the live GitHub PR conversation. Tests replace this.
var getGitHubPRSnapshot = fetchGitHubPRSnapshot

// postGitHubPRCommentFn posts a conversation or inline comment. Tests replace this.
var postGitHubPRCommentFn = postGitHubPRComment

// submitGitHubPRReviewFn submits a GH review. Tests replace this.
var submitGitHubPRReviewFn = submitGitHubPRReview

// editGitHubPRFn updates a GH PR title/body. Tests replace this.
var editGitHubPRFn = editGitHubPR

// omitPREditBody tells editGitHubPR to send --title only.
const omitPREditBody = "\x00omit-pr-edit-body"

// editGitHubPRCommentFn updates a GH conversation or review comment. Tests replace this.
var editGitHubPRCommentFn = editGitHubPRComment

// deleteGitHubPRCommentFn deletes a GH line note or issue comment. Tests replace this.
var deleteGitHubPRCommentFn = deleteGitHubPRComment

// replyGitHubPRCommentFn posts a reply on a GH line-note thread and returns the
// created comment JSON. Tests replace this.
var replyGitHubPRCommentFn = replyGitHubPRComment

// setGitHubReviewThreadFn resolves or unresolves a GH review thread. Tests replace this.
var setGitHubReviewThreadFn = setGitHubReviewThread

// githubViewerFn loads the authenticated user, repo write bit, and allowed
// merge methods. Tests replace this.
var githubViewerFn = fetchGitHubViewer

// setGitHubPRDraftFn marks a PR ready or converts it back to draft. Tests replace this.
var setGitHubPRDraftFn = setGitHubPRDraft

// ghPRSnapshot is the Recur-facing view of a GitHub PR conversation.
type ghPRSnapshot struct {
	Title               string
	State               string // OPEN, MERGED, CLOSED
	ReviewDecision      string
	Mergeable           string
	MergeStateStatus    string
	IsDraft             bool
	HeadRefOid          string
	HeadRefName         string
	BaseRefName         string
	URL                 string
	AuthorLogin         string
	ViewerLogin         string
	ViewerCanWrite      bool
	AllowedMergeMethods []string
	NodeID              string
	CreatedAt           string // GitHub PR opened-at (RFC3339)
	UpdatedAt           string // GitHub PR last-activity (RFC3339)
	ReviewRequests      []ghListedActor
	Assignees           []ghListedActor
	Comments            []domain.PRComment
	viewCommits         []ghViewCommit
	commitAuthors       map[string]ghViewActor // oid → GitHub user (from gh pr view commits)
	commitMeta          map[string]ghCommitMeta
	authorAvatars       map[string]string // login/display → GitHub avatar (app /in/ for bots)
	// simplify: last:100 snapshot; keep persisted issue comments/reviews when
	// pageInfo.hasPreviousPage. Paginate those connections for first-load of
	// threads over 100 rows.
	commentsTruncated bool
	reviewsTruncated  bool
}

type ghPRViewJSON struct {
	ID               string          `json:"id"`
	Title            string          `json:"title"`
	Body             string          `json:"body"`
	State            string          `json:"state"`
	ReviewDecision   string          `json:"reviewDecision"`
	Mergeable        string          `json:"mergeable"`
	MergeStateStatus string          `json:"mergeStateStatus"`
	IsDraft          bool            `json:"isDraft"`
	HeadRefOid       string          `json:"headRefOid"`
	HeadRefName      string          `json:"headRefName"`
	BaseRefName      string          `json:"baseRefName"`
	URL              string          `json:"url"`
	CreatedAt        string          `json:"createdAt"`
	UpdatedAt        string          `json:"updatedAt"`
	Author           ghViewActor     `json:"author"`
	Comments         []ghViewComment `json:"comments"`
	Reviews          []ghViewReview  `json:"reviews"`
	Commits          []ghViewCommit  `json:"commits"`
	ReviewRequests   []ghListedActor `json:"reviewRequests"`
	Assignees        []ghListedActor `json:"assignees"`
}

type ghViewCommit struct {
	OID             string               `json:"oid"`
	MessageHeadline string               `json:"messageHeadline"`
	MessageBody     string               `json:"messageBody"`
	CommittedDate   string               `json:"committedDate"`
	Authors         []ghViewCommitAuthor `json:"authors"`
}

type ghViewCommitAuthor struct {
	Login string `json:"login"`
	Name  string `json:"name"`
}

type ghViewActor struct {
	Login          string `json:"login"`
	Type           string `json:"type"`
	AvatarURL      string `json:"avatarUrl"`
	AvatarURLSnake string `json:"avatar_url"`
}

func (a ghViewActor) avatar() string {
	if u := strings.TrimSpace(a.AvatarURL); u != "" {
		return u
	}
	return strings.TrimSpace(a.AvatarURLSnake)
}

type ghViewComment struct {
	ID        string      `json:"id"`
	URL       string      `json:"url"`
	Author    ghViewActor `json:"author"`
	Body      string      `json:"body"`
	CreatedAt string      `json:"createdAt"`
}

type ghViewReview struct {
	ID          string      `json:"id"`
	URL         string      `json:"url"`
	Author      ghViewActor `json:"author"`
	Body        string      `json:"body"`
	State       string      `json:"state"`
	SubmittedAt string      `json:"submittedAt"`
}

type ghInlineComment struct {
	ID                  int64       `json:"id"`
	HTMLURL             string      `json:"html_url"`
	Body                string      `json:"body"`
	Path                string      `json:"path"`
	Line                *int        `json:"line"`
	OriginalLine        int         `json:"original_line"`
	StartLine           *int        `json:"start_line"`
	OriginalStartLine   int         `json:"original_start_line"`
	Side                string      `json:"side"`
	StartSide           string      `json:"start_side"`
	DiffHunk            string      `json:"diff_hunk"`
	CreatedAt           string      `json:"created_at"`
	PullRequestReviewID int64       `json:"pull_request_review_id"`
	InReplyToID         int64       `json:"in_reply_to_id"`
	User                ghViewActor `json:"user"`
}

func (row ghInlineComment) commentLine() int {
	if row.Line != nil && *row.Line > 0 {
		return *row.Line
	}
	return row.OriginalLine
}

func (row ghInlineComment) commentOutdated() bool {
	return row.Line == nil
}

func (row ghInlineComment) commentStartLine() int {
	start := 0
	if row.StartLine != nil {
		start = *row.StartLine
	}
	if start <= 0 {
		start = row.OriginalStartLine
	}
	end := row.commentLine()
	if start <= 0 || start >= end {
		return 0
	}
	return start
}

func parseGitHubPRView(raw string, now time.Time) (ghPRSnapshot, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ghPRSnapshot{}, fmt.Errorf("empty gh pr view json")
	}
	var view ghPRViewJSON
	if err := json.Unmarshal([]byte(raw), &view); err != nil {
		return ghPRSnapshot{}, fmt.Errorf("parse gh pr view json: %w", err)
	}
	return parseGitHubPRViewJSON(view, now), nil
}

func parseGitHubPRViewJSON(view ghPRViewJSON, now time.Time) ghPRSnapshot {
	snap := ghPRSnapshot{
		Title:            strings.TrimSpace(view.Title),
		State:            strings.ToUpper(strings.TrimSpace(view.State)),
		ReviewDecision:   strings.ToUpper(strings.TrimSpace(view.ReviewDecision)),
		Mergeable:        strings.ToUpper(strings.TrimSpace(view.Mergeable)),
		MergeStateStatus: strings.ToUpper(strings.TrimSpace(view.MergeStateStatus)),
		IsDraft:          view.IsDraft,
		HeadRefOid:       strings.TrimSpace(view.HeadRefOid),
		HeadRefName:      strings.TrimSpace(view.HeadRefName),
		BaseRefName:      strings.TrimSpace(view.BaseRefName),
		URL:              strings.TrimSpace(view.URL),
		AuthorLogin:      strings.TrimSpace(view.Author.Login),
		NodeID:           strings.TrimSpace(view.ID),
		CreatedAt:        strings.TrimSpace(view.CreatedAt),
		UpdatedAt:        strings.TrimSpace(view.UpdatedAt),
		commitAuthors:    commitAuthorsFromView(view.Commits),
		viewCommits:      view.Commits,
		ReviewRequests:   view.ReviewRequests,
		Assignees:        view.Assignees,
	}
	opened := strings.TrimSpace(view.CreatedAt)
	if opened == "" {
		opened = now.Format(time.RFC3339)
	}
	snap.Comments = append(snap.Comments, ghDescriptionComment(view.Author, view.Body, opened, snap.URL, view.ID, now))
	for _, c := range view.Comments {
		id := strings.TrimSpace(c.ID)
		if id == "" {
			id = fmt.Sprintf("%s-%s", c.CreatedAt, c.Author.Login)
		}
		row := ghThreadComment(
			"gh-comment-"+id, c.Author, c.Body, "", c.CreatedAt, now, "comment")
		row.NodeID = id
		row.URL = githubPermalink(snap.URL, "issuecomment", id, c.URL)
		snap.Comments = append(snap.Comments, row)
	}
	for _, r := range view.Reviews {
		if strings.EqualFold(r.State, "PENDING") {
			continue
		}
		id := strings.TrimSpace(r.ID)
		if id == "" {
			id = fmt.Sprintf("%s-%s-%s", r.SubmittedAt, r.Author.Login, r.State)
		}
		when := r.SubmittedAt
		if when == "" {
			when = now.Format(time.RFC3339)
		}
		url := githubPermalink(snap.URL, "pullrequestreview", id, r.URL)
		if body := strings.TrimSpace(r.Body); body != "" {
			row := ghThreadComment(
				"gh-review-"+id, r.Author, body, "", when, now, "comment")
			row.NodeID = id
			row.URL = url
			row.Verb = ghReviewCardVerb(r.State)
			snap.Comments = append(snap.Comments, row)
			continue
		}
		act := ghActionComment("gh-review-action-"+id, r.Author, ghReviewAction(r.State), when, now)
		act.NodeID = id
		act.URL = url
		act.Verb = ghReviewCardVerb(r.State)
		snap.Comments = append(snap.Comments, act)
	}
	return snap
}

func parseGitHubInlineComments(raw string, now time.Time, prURL string) []domain.PRComment {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "[]" {
		return nil
	}
	var rows []ghInlineComment
	if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil
	}
	return commentsFromInlineRows(rows, now, prURL)
}

func commentsFromInlineRows(rows []ghInlineComment, now time.Time, prURL string) []domain.PRComment {
	base := strings.TrimRight(strings.TrimSpace(prURL), "/")
	out := make([]domain.PRComment, 0, len(rows))
	for _, row := range rows {
		path := strings.TrimSpace(row.Path)
		line := row.commentLine()
		start := row.commentStartLine()
		outdated := row.commentOutdated()
		loc := path
		if loc != "" && line > 0 {
			if start > 0 {
				loc = fmt.Sprintf("%s:%d-%d", loc, start, line)
			} else {
				loc = fmt.Sprintf("%s:%d", loc, line)
			}
		}
		if outdated && loc != "" {
			loc += " (outdated)"
		}
		c := ghThreadComment(
			fmt.Sprintf("gh-inline-%d", row.ID), row.User, row.Body, loc, row.CreatedAt, now, "comment")
		c.URL = strings.TrimSpace(row.HTMLURL)
		if c.URL == "" && base != "" && row.ID > 0 {
			c.URL = fmt.Sprintf("%s#discussion_r%d", base, row.ID)
		}
		c.Path = path
		c.Line = line
		c.StartLine = start
		c.Side = strings.TrimSpace(row.Side)
		c.StartSide = strings.TrimSpace(row.StartSide)
		c.Outdated = outdated
		c.DiffHunk = strings.TrimSpace(row.DiffHunk)
		if row.PullRequestReviewID > 0 {
			c.ReviewID = strconv.FormatInt(row.PullRequestReviewID, 10)
		}
		if row.InReplyToID > 0 {
			c.InReplyToID = fmt.Sprintf("gh-inline-%d", row.InReplyToID)
		}
		out = append(out, c)
	}
	return out
}

func githubPermalink(prURL, fragment, nodeID, existing string) string {
	if u := strings.TrimSpace(existing); u != "" {
		return u
	}
	base := strings.TrimRight(strings.TrimSpace(prURL), "/")
	if base == "" {
		return ""
	}
	db, ok := githubNodeDatabaseID(nodeID)
	if !ok {
		return ""
	}
	return fmt.Sprintf("%s#%s-%d", base, fragment, db)
}

func githubNodeDatabaseID(id string) (int64, bool) {
	id = strings.TrimSpace(id)
	if id == "" {
		return 0, false
	}
	if n, err := strconv.ParseInt(id, 10, 64); err == nil && n > 0 {
		return n, true
	}
	if i := strings.IndexByte(id, '_'); i > 0 && i < len(id)-1 && githubNodePrefix(id[:i]) {
		raw, ok := decodeGitHubNodePayload(id[i+1:])
		if !ok || len(raw) < 5 {
			return 0, false
		}
		var n uint64
		for _, b := range raw[4:] {
			n = n<<8 | uint64(b)
		}
		return int64(n), n > 0
	}
	raw, ok := decodeGitHubNodePayload(id)
	if !ok {
		return 0, false
	}
	return githubLegacyNodeID(raw)
}

func githubNodePrefix(s string) bool {
	if s == "" || len(s) > 8 {
		return false
	}
	for _, c := range s {
		if c < 'A' || c > 'Z' {
			return false
		}
	}
	return true
}

func decodeGitHubNodePayload(payload string) ([]byte, bool) {
	payload = strings.TrimSpace(payload)
	if payload == "" {
		return nil, false
	}
	if m := len(payload) % 4; m != 0 {
		payload += strings.Repeat("=", 4-m)
	}
	if raw, err := base64.URLEncoding.DecodeString(payload); err == nil && len(raw) > 0 {
		return raw, true
	}
	raw, err := base64.StdEncoding.DecodeString(payload)
	return raw, err == nil && len(raw) > 0
}

func githubLegacyNodeID(raw []byte) (int64, bool) {
	s := string(raw)
	i := strings.IndexByte(s, ':')
	if i < 0 || i == len(s)-1 {
		return 0, false
	}
	rest := s[i+1:]
	j := 0
	for j < len(rest) && ((rest[j] >= 'A' && rest[j] <= 'Z') || (rest[j] >= 'a' && rest[j] <= 'z')) {
		j++
	}
	if j == 0 || j == len(rest) {
		return 0, false
	}
	n, err := strconv.ParseInt(rest[j:], 10, 64)
	return n, err == nil && n > 0
}

func ghDescriptionComment(author ghViewActor, body, when, prURL, nodeID string, now time.Time) domain.PRComment {
	c := ghThreadComment("gh-description", author, body, "", when, now, "description")
	c.NodeID = strings.TrimSpace(nodeID)
	c.URL = githubPermalink(prURL, "issue", nodeID, "")
	if c.URL == "" {
		c.URL = strings.TrimSpace(prURL)
	}
	if strings.TrimSpace(body) == "" {
		c.BodyMD = ""
	}
	return c
}

func ghActionComment(id string, author ghViewActor, verb, when string, now time.Time) domain.PRComment {
	return ghThreadComment(id, author, verb, "", when, now, "action")
}

func ghThreadComment(id string, author ghViewActor, body, loc, when string, now time.Time, kind string) domain.PRComment {
	login := strings.TrimSpace(author.Login)
	if login == "" {
		login = "someone"
	}
	display := ghDisplayName(login)
	md := strings.TrimSpace(body)
	locHTML := ""
	if loc != "" {
		locHTML = "<code>" + html.EscapeString(loc) + "</code>"
	}
	if kind == "" {
		kind = "comment"
	}
	authorLogin := login
	if strings.EqualFold(login, "someone") {
		authorLogin = ""
	}
	verb := ""
	if kind == "comment" {
		verb = "commented"
	}
	return domain.PRComment{
		ID:          id,
		Code:        ghAuthorCode(display),
		Author:      display,
		AuthorLogin: authorLogin,
		AvatarCSS:   commentAvatarCSS("AR"),
		Time:        formatGHWhen(when, now),
		At:          strings.TrimSpace(when),
		Kind:        kind,
		Verb:        verb,
		BodyMD:      md,
		BodyHTML:    locHTML,
		AvatarURL:   ghAvatarURL(login, author.avatar()),
	}
}

func ghAvatarURL(login, provided string) string {
	if u := strings.TrimSpace(provided); u != "" {
		return u
	}
	login = strings.TrimSpace(login)
	login = strings.TrimSuffix(login, "[bot]")
	if login == "" || strings.EqualFold(login, "someone") {
		return ""
	}
	for _, r := range login {
		ok := r == '-' || r == '_' || (r >= '0' && r <= '9') || (r >= 'A' && r <= 'Z') || (r >= 'a' && r <= 'z')
		if !ok {
			return ""
		}
	}
	return "https://github.com/" + login + ".png?size=56"
}

func rememberGHAvatar(m map[string]string, a ghViewActor) {
	if m == nil {
		return
	}
	url := a.avatar()
	login := strings.TrimSpace(a.Login)
	if login == "" || url == "" {
		return
	}
	m[strings.ToLower(login)] = url
	if display := strings.ToLower(ghDisplayName(login)); display != "" {
		m[display] = url
	}
	if trimmed := strings.ToLower(strings.TrimSuffix(login, "[bot]")); trimmed != "" {
		m[trimmed] = url
	}
}

func applyGHAvatars(comments []domain.PRComment, avatars map[string]string) {
	if len(avatars) == 0 {
		return
	}
	for i := range comments {
		if strings.TrimSpace(comments[i].AvatarURL) == "" {
			continue
		}
		for _, key := range []string{comments[i].AuthorLogin, comments[i].Author} {
			if url := avatars[strings.ToLower(strings.TrimSpace(key))]; url != "" {
				comments[i].AvatarURL = url
				break
			}
		}
	}
}

func applyGHCommitAvatars(commits []domain.Commit, avatars map[string]string) {
	if len(avatars) == 0 {
		return
	}
	for i := range commits {
		if strings.TrimSpace(commits[i].AvatarURL) == "" {
			continue
		}
		if url := avatars[strings.ToLower(strings.TrimSpace(commits[i].Author))]; url != "" {
			commits[i].AvatarURL = url
		}
	}
}

func ghAuthorCode(login string) string {
	r := []rune(strings.ToUpper(strings.TrimSpace(login)))
	switch len(r) {
	case 0:
		return "GH"
	case 1:
		return string(r[0])
	default:
		return string(r[0]) + string(r[1])
	}
}

func ghReviewAction(state string) string {
	switch strings.ToUpper(strings.TrimSpace(state)) {
	case "APPROVED":
		return "approved"
	case "CHANGES_REQUESTED":
		return "requested changes"
	case "DISMISSED":
		return "dismissed a review"
	default:
		return "reviewed"
	}
}

func ghReviewCardVerb(state string) string {
	switch strings.ToUpper(strings.TrimSpace(state)) {
	case "APPROVED":
		return "approved"
	case "CHANGES_REQUESTED":
		return "requested changes"
	case "DISMISSED":
		return "dismissed a review"
	default:
		return "commented"
	}
}

func ghDisplayName(login string) string {
	login = strings.TrimSpace(login)
	if strings.Contains(strings.ToLower(login), "copilot") {
		return "Copilot"
	}
	return strings.TrimSuffix(login, "[bot]")
}

type ghTimelineEvent struct {
	ID                int64        `json:"id"`
	HTMLURL           string       `json:"html_url"`
	Event             string       `json:"event"`
	CreatedAt         string       `json:"created_at"`
	SubmittedAt       string       `json:"submitted_at"`
	SHA               string       `json:"sha"`
	CommitID          string       `json:"commit_id"`
	Message           string       `json:"message"`
	Actor             *ghViewActor `json:"actor"`
	User              *ghViewActor `json:"user"`
	ReviewRequester   *ghViewActor `json:"review_requester"`
	RequestedReviewer *ghViewActor `json:"requested_reviewer"`
	RequestedTeam     *struct {
		Name string `json:"name"`
	} `json:"requested_team"`
	Author *struct {
		Name string `json:"name"`
		Date string `json:"date"`
	} `json:"author"`
	Label *struct {
		Name string `json:"name"`
	} `json:"label"`
	Rename *struct {
		From string `json:"from"`
		To   string `json:"to"`
	} `json:"rename"`
	Assignee              *ghViewActor `json:"assignee"`
	PerformedViaGitHubApp *struct {
		Slug string `json:"slug"`
	} `json:"performed_via_github_app"`
}

func commitAuthorsFromView(commits []ghViewCommit) map[string]ghViewActor {
	m := make(map[string]ghViewActor)
	for _, c := range commits {
		oid := strings.TrimSpace(c.OID)
		if oid == "" {
			continue
		}
		for _, a := range c.Authors {
			if login := strings.TrimSpace(a.Login); login != "" {
				m[oid] = ghViewActor{Login: login}
				break
			}
		}
	}
	return m
}

func lookupCommitAuthor(authors map[string]ghViewActor, sha string) (ghViewActor, bool) {
	sha = strings.TrimSpace(sha)
	if sha == "" || len(authors) == 0 {
		return ghViewActor{}, false
	}
	if a, ok := authors[sha]; ok {
		return a, true
	}
	if len(sha) >= 7 {
		for oid, a := range authors {
			if strings.HasPrefix(oid, sha) || strings.HasPrefix(sha, oid) {
				return a, true
			}
		}
	}
	return ghViewActor{}, false
}

type ghCommitMeta struct {
	Actor       ghViewActor
	Verified    bool
	CheckStatus string
	URL         string
}

func lookupCommitMeta(meta map[string]ghCommitMeta, sha string) (ghCommitMeta, bool) {
	sha = strings.TrimSpace(sha)
	if sha == "" || len(meta) == 0 {
		return ghCommitMeta{}, false
	}
	if m, ok := meta[sha]; ok {
		return m, true
	}
	if len(sha) >= 7 {
		for oid, m := range meta {
			if strings.HasPrefix(oid, sha) || strings.HasPrefix(sha, oid) {
				return m, true
			}
		}
	}
	return ghCommitMeta{}, false
}

func fetchGitHubCommitRollup(ctx context.Context, repoPath, ownerRepo string, prNumber int) (map[string]ghCommitMeta, map[string]string) {
	owner, repo, ok := splitOwnerRepo(ownerRepo)
	if !ok || prNumber <= 0 {
		return nil, nil
	}
	const q = `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){author{login avatarUrl}comments(last:100){nodes{author{login avatarUrl}}}reviews(last:100){nodes{author{login avatarUrl}}}commits(last:100){nodes{commit{oid url signature{isValid} statusCheckRollup{state}}}}}}}`
	out, err := runGH(ctx, repoPath, "api", "graphql",
		"-f", "query="+q,
		"-F", "o="+owner,
		"-F", "r="+repo,
		"-F", "n="+strconv.Itoa(prNumber))
	if err != nil {
		return nil, nil
	}
	return parseGitHubCommitRollup(out)
}

func splitOwnerRepo(ownerRepo string) (owner, repo string, ok bool) {
	ownerRepo = strings.TrimSpace(ownerRepo)
	i := strings.IndexByte(ownerRepo, '/')
	if i <= 0 || i >= len(ownerRepo)-1 {
		return "", "", false
	}
	return ownerRepo[:i], ownerRepo[i+1:], true
}

func parseGitHubCommitRollup(raw string) (map[string]ghCommitMeta, map[string]string) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, nil
	}
	var payload struct {
		Data struct {
			Repository struct {
				PullRequest struct {
					Author   ghViewActor `json:"author"`
					Comments struct {
						Nodes []struct {
							Author ghViewActor `json:"author"`
						} `json:"nodes"`
					} `json:"comments"`
					Reviews struct {
						Nodes []struct {
							Author ghViewActor `json:"author"`
						} `json:"nodes"`
					} `json:"reviews"`
					Commits struct {
						Nodes []struct {
							Commit struct {
								OID       string `json:"oid"`
								URL       string `json:"url"`
								Signature *struct {
									IsValid bool `json:"isValid"`
								} `json:"signature"`
								StatusCheckRollup *struct {
									State string `json:"state"`
								} `json:"statusCheckRollup"`
							} `json:"commit"`
						} `json:"nodes"`
					} `json:"commits"`
				} `json:"pullRequest"`
			} `json:"repository"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(raw), &payload); err != nil {
		return nil, nil
	}
	pr := payload.Data.Repository.PullRequest
	avatars := make(map[string]string)
	rememberGHAvatar(avatars, pr.Author)
	for _, n := range pr.Comments.Nodes {
		rememberGHAvatar(avatars, n.Author)
	}
	for _, n := range pr.Reviews.Nodes {
		rememberGHAvatar(avatars, n.Author)
	}
	if len(avatars) == 0 {
		avatars = nil
	}
	out := make(map[string]ghCommitMeta)
	for _, n := range pr.Commits.Nodes {
		oid := strings.TrimSpace(n.Commit.OID)
		if oid == "" {
			continue
		}
		m := ghCommitMeta{URL: strings.TrimSpace(n.Commit.URL)}
		if n.Commit.Signature != nil {
			m.Verified = n.Commit.Signature.IsValid
		}
		if n.Commit.StatusCheckRollup != nil {
			m.CheckStatus = githubRollupStatus(n.Commit.StatusCheckRollup.State)
		}
		out[oid] = m
	}
	if len(out) == 0 {
		return nil, avatars
	}
	return out, avatars
}

func parseMentionableUsers(raw string) []domain.PRParticipant {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil
	}
	var payload struct {
		Data struct {
			Repository struct {
				MentionableUsers struct {
					Nodes []struct {
						Login     string `json:"login"`
						Name      string `json:"name"`
						AvatarURL string `json:"avatarUrl"`
					} `json:"nodes"`
				} `json:"mentionableUsers"`
			} `json:"repository"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(raw), &payload); err != nil {
		return nil
	}
	var out []domain.PRParticipant
	for _, n := range payload.Data.Repository.MentionableUsers.Nodes {
		login := strings.TrimSpace(n.Login)
		if login == "" {
			continue
		}
		out = append(out, domain.PRParticipant{
			Login:     login,
			Name:      strings.TrimSpace(n.Name),
			AvatarURL: ghAvatarURL(login, strings.TrimSpace(n.AvatarURL)),
		})
	}
	return out
}

func (e *Engine) SearchPRMentionableUsers(prID, query string) []domain.PRParticipant {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub {
		return nil
	}
	ownerRepo, ok := ownerRepoFromPRURL(pr.URL)
	if !ok {
		return nil
	}
	owner, repo, ok := splitOwnerRepo(ownerRepo)
	if !ok {
		return nil
	}
	query = strings.TrimSpace(query)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	const q = `query($o:String!,$r:String!,$q:String){repository(owner:$o,name:$r){mentionableUsers(first:10,query:$q){nodes{login name avatarUrl}}}}`
	out, err := runGH(ctx, e.prRepoPath(pr), "api", "graphql",
		"-f", "query="+q,
		"-f", "o="+owner,
		"-f", "r="+repo,
		"-f", "q="+query)
	if err != nil {
		return nil
	}
	return parseMentionableUsers(out)
}

func githubRollupStatus(state string) string {
	switch strings.ToUpper(strings.TrimSpace(state)) {
	case "SUCCESS":
		return "pass"
	case "FAILURE", "ERROR":
		return "fail"
	case "PENDING", "EXPECTED":
		return "pending"
	default:
		return ""
	}
}

func timelineEventURL(ev ghTimelineEvent, prURL, commitURL string) string {
	if u := strings.TrimSpace(ev.HTMLURL); u != "" {
		return u
	}
	if u := strings.TrimSpace(commitURL); u != "" {
		return u
	}
	base := strings.TrimRight(strings.TrimSpace(prURL), "/")
	if base == "" || ev.ID <= 0 {
		return ""
	}
	return fmt.Sprintf("%s#event-%d", base, ev.ID)
}

func withTimelineURL(c domain.PRComment, ev ghTimelineEvent, prURL, commitURL string) domain.PRComment {
	if strings.TrimSpace(c.URL) == "" {
		c.URL = timelineEventURL(ev, prURL, commitURL)
	}
	return c
}

func parseGitHubTimeline(raw string, now time.Time, authors map[string]ghViewActor, meta map[string]ghCommitMeta, prURL, head, base string) []domain.PRComment {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "[]" {
		return nil
	}
	var rows []ghTimelineEvent
	if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil
	}
	hasMerged := false
	for _, ev := range rows {
		if strings.EqualFold(strings.TrimSpace(ev.Event), "merged") {
			hasMerged = true
			break
		}
	}
	out := make([]domain.PRComment, 0)
	for i, ev := range rows {
		kind := strings.ToLower(strings.TrimSpace(ev.Event))
		when := ev.CreatedAt
		if when == "" {
			when = ev.SubmittedAt
		}
		if ev.Author != nil && ev.Author.Date != "" && when == "" {
			when = ev.Author.Date
		}
		id := fmt.Sprintf("gh-event-%s-%d-%s", kind, i, when)
		switch kind {
		case "committed":
			who := ghViewActor{Login: "someone"}
			if ev.Author != nil && ev.Author.Name != "" {
				who = ghViewActor{Login: ev.Author.Name}
			}
			linked, linkedOK := lookupCommitAuthor(authors, ev.SHA)
			if linkedOK && linked.Login != "" {
				who = linked
			}
			msg := strings.TrimSpace(ev.Message)
			if i := strings.IndexByte(msg, '\n'); i >= 0 {
				msg = strings.TrimSpace(msg[:i])
			}
			if msg == "" {
				msg = ev.SHA
			}
			act := ghActionComment(id, who, "committed "+msg, when, now)
			if !linkedOK || linked.Login == "" {
				act.AvatarURL = ""
			}
			act.CommitSHA = strings.TrimSpace(ev.SHA)
			commitURL := ""
			if cm, ok := lookupCommitMeta(meta, ev.SHA); ok {
				act.Verified = cm.Verified
				act.CheckStatus = cm.CheckStatus
				act.URL = cm.URL
				commitURL = cm.URL
			}
			out = append(out, withTimelineURL(act, ev, prURL, commitURL))
		case "review_requested":
			who := timelineActor(ev.ReviewRequester, ev.Actor)
			target := "a reviewer"
			if ev.RequestedReviewer != nil && ev.RequestedReviewer.Login != "" {
				target = ghDisplayName(ev.RequestedReviewer.Login)
			} else if ev.RequestedTeam != nil && ev.RequestedTeam.Name != "" {
				target = ev.RequestedTeam.Name
			}
			out = append(out, withTimelineURL(ghActionComment(id, who, "requested a review from "+target, when, now), ev, prURL, ""))
		case "review_request_removed":
			who := timelineActor(ev.ReviewRequester, ev.Actor)
			target := "a reviewer"
			if ev.RequestedReviewer != nil && ev.RequestedReviewer.Login != "" {
				target = ghDisplayName(ev.RequestedReviewer.Login)
			}
			out = append(out, withTimelineURL(ghActionComment(id, who, "removed the review request for "+target, when, now), ev, prURL, ""))
		case "ready_for_review":
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "marked this pull request as ready for review", when, now), ev, prURL, ""))
		case "convert_to_draft":
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "converted this pull request to draft", when, now), ev, prURL, ""))
		case "closed":
			if hasMerged || strings.TrimSpace(ev.CommitID) != "" {
				continue
			}
			out = append(out, glyphTimelineAction(id, ev, "closed this", when, now, prURL))
		case "reopened":
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "reopened this", when, now), ev, prURL, ""))
		case "merged":
			act := glyphTimelineAction(id, ev, mergeEventBody(ev, base), when, now, prURL)
			act.CommitSHA = timelineSHA(ev)
			out = append(out, act)
		case "head_ref_deleted":
			out = append(out, glyphTimelineAction(id, ev, headRefEventBody("deleted", head), when, now, prURL))
		case "head_ref_restored":
			out = append(out, glyphTimelineAction(id, ev, headRefEventBody("restored", head), when, now, prURL))
		case "head_ref_force_pushed":
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "force-pushed the branch", when, now), ev, prURL, ""))
		case "assigned":
			who := timelineActor(ev.Actor, nil)
			target := "someone"
			if ev.Assignee != nil && ev.Assignee.Login != "" {
				target = ghDisplayName(ev.Assignee.Login)
			}
			out = append(out, withTimelineURL(ghActionComment(id, who, "assigned "+target, when, now), ev, prURL, ""))
		case "unassigned":
			who := timelineActor(ev.Actor, nil)
			target := "someone"
			if ev.Assignee != nil && ev.Assignee.Login != "" {
				target = ghDisplayName(ev.Assignee.Login)
			}
			out = append(out, withTimelineURL(ghActionComment(id, who, "unassigned "+target, when, now), ev, prURL, ""))
		case "labeled":
			label := "a label"
			if ev.Label != nil && strings.TrimSpace(ev.Label.Name) != "" {
				label = ev.Label.Name
			}
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "added the "+label+" label", when, now), ev, prURL, ""))
		case "unlabeled":
			label := "a label"
			if ev.Label != nil && strings.TrimSpace(ev.Label.Name) != "" {
				label = ev.Label.Name
			}
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), "removed the "+label+" label", when, now), ev, prURL, ""))
		case "renamed":
			body := "changed the title"
			if ev.Rename != nil && ev.Rename.From != "" && ev.Rename.To != "" {
				body = "changed the title from " + ev.Rename.From + " to " + ev.Rename.To
			}
			out = append(out, withTimelineURL(ghActionComment(id, timelineActor(ev.Actor, nil), body, when, now), ev, prURL, ""))
		case "copilot_work_started", "copilot_work_finished", "copilot_work_finished_failure":
			if act, ok := copilotWorkAction(id, kind, ev, when, now); ok {
				out = append(out, withTimelineURL(act, ev, prURL, ""))
			}
		}
	}
	return out
}

func glyphTimelineAction(id string, ev ghTimelineEvent, body, when string, now time.Time, prURL string) domain.PRComment {
	act := ghActionComment(id, timelineActor(ev.Actor, nil), body, when, now)
	act.AvatarURL = ""
	return withTimelineURL(act, ev, prURL, "")
}

func timelineSHA(ev ghTimelineEvent) string {
	if s := strings.TrimSpace(ev.SHA); s != "" {
		return s
	}
	return strings.TrimSpace(ev.CommitID)
}

func shortGitSHA(sha string) string {
	sha = strings.TrimSpace(sha)
	if len(sha) > 7 {
		return sha[:7]
	}
	return sha
}

func mergeEventBody(ev ghTimelineEvent, base string) string {
	sha := shortGitSHA(timelineSHA(ev))
	base = strings.TrimSpace(base)
	switch {
	case sha != "" && base != "":
		return "merged commit " + sha + " into " + base
	case sha != "":
		return "merged commit " + sha
	case base != "":
		return "merged this into " + base
	default:
		return "merged this pull request"
	}
}

func headRefEventBody(verb, head string) string {
	head = strings.TrimSpace(head)
	if head != "" {
		return verb + " the " + head + " branch"
	}
	return verb + " the head branch"
}

func copilotWorkAction(id, kind string, ev ghTimelineEvent, when string, now time.Time) (domain.PRComment, bool) {
	behalf := "someone"
	if ev.Actor != nil && strings.TrimSpace(ev.Actor.Login) != "" {
		behalf = ghDisplayName(ev.Actor.Login)
	}
	review := copilotAppSlug(ev) == "copilot-pull-request-reviewer"
	var verb string
	switch kind {
	case "copilot_work_started":
		if review {
			verb = "started reviewing on behalf of " + behalf
		} else {
			verb = "started work on behalf of " + behalf
		}
	case "copilot_work_finished":
		if review {
			verb = "finished reviewing on behalf of " + behalf
		} else {
			verb = "finished work on behalf of " + behalf
		}
	case "copilot_work_finished_failure":
		verb = "stopped work on behalf of " + behalf + " due to an error"
	default:
		return domain.PRComment{}, false
	}
	act := ghActionComment(id, ghViewActor{Login: "copilot-pull-request-reviewer", Type: "Bot"}, verb, when, now)
	// GitHub uses the branch glyph here, not Copilot's avatar.
	act.AvatarURL = ""
	return act, true
}

func copilotAppSlug(ev ghTimelineEvent) string {
	if ev.PerformedViaGitHubApp == nil {
		return ""
	}
	return strings.ToLower(strings.TrimSpace(ev.PerformedViaGitHubApp.Slug))
}

func timelineActor(primary, fallback *ghViewActor) ghViewActor {
	if primary != nil && strings.TrimSpace(primary.Login) != "" {
		return *primary
	}
	if fallback != nil {
		return *fallback
	}
	return ghViewActor{Login: "someone"}
}

// attachReviewComments nests inline notes under the review that submitted them
// (GitHub's Conversation tab: review body + line comments as one unit).
func attachReviewComments(cs []domain.PRComment) {
	parents := map[string]int{}
	byID := map[string]int{}
	for i, c := range cs {
		byID[c.ID] = i
		if strings.HasPrefix(c.ID, "gh-inline-") {
			byID[strings.TrimPrefix(c.ID, "gh-inline-")] = i
		}
		if !strings.HasPrefix(c.ID, "gh-review-") {
			continue
		}
		for _, key := range reviewMatchKeys(c) {
			parents[key] = i
		}
	}
	hasKids := map[int]bool{}
	for i, c := range cs {
		if !strings.HasPrefix(c.ID, "gh-inline-") || c.InReplyToID != "" {
			continue
		}
		pi, ok := parents[strings.TrimSpace(c.ReviewID)]
		if !ok {
			continue
		}
		cs[i].ParentID = cs[pi].ID
		hasKids[pi] = true
	}
	for i, c := range cs {
		if !strings.HasPrefix(c.ID, "gh-inline-") || c.InReplyToID == "" {
			continue
		}
		root := resolveInlineRoot(byID, cs, c.InReplyToID)
		if root != "" {
			cs[i].InReplyToID = root
			if ri, ok := byID[root]; ok {
				cs[i].ParentID = cs[ri].ParentID
			}
		}
	}
	for pi := range hasKids {
		promoteReviewWithKids(&cs[pi], cs)
	}
	hideReplyOnlyReviewActions(cs)
}

func resolveInlineRoot(byID map[string]int, cs []domain.PRComment, id string) string {
	seen := map[string]bool{}
	cur := strings.TrimSpace(id)
	for cur != "" && !seen[cur] {
		seen[cur] = true
		i, ok := byID[cur]
		if !ok {
			i, ok = byID["gh-inline-"+strings.TrimPrefix(cur, "gh-inline-")]
		}
		if !ok {
			if strings.HasPrefix(cur, "gh-inline-") {
				return cur
			}
			return "gh-inline-" + cur
		}
		if cs[i].InReplyToID == "" {
			return cs[i].ID
		}
		cur = cs[i].InReplyToID
	}
	return cur
}

func hideReplyOnlyReviewActions(cs []domain.PRComment) {
	hasRoot := map[string]bool{}
	hasReply := map[string]bool{}
	for _, c := range cs {
		if !strings.HasPrefix(c.ID, "gh-inline-") || c.ReviewID == "" {
			continue
		}
		if c.InReplyToID != "" {
			hasReply[c.ReviewID] = true
		} else {
			hasRoot[c.ReviewID] = true
		}
	}
	for i, c := range cs {
		if c.Kind != "action" || !strings.HasPrefix(c.ID, "gh-review-") {
			continue
		}
		if c.Verb != "commented" && c.BodyMD != "reviewed" {
			continue
		}
		onlyReplies := false
		for _, key := range reviewMatchKeys(c) {
			if hasRoot[key] {
				onlyReplies = false
				break
			}
			if hasReply[key] {
				onlyReplies = true
			}
		}
		if onlyReplies {
			cs[i].Kind = "hidden"
		}
	}
}

func reviewMatchKeys(c domain.PRComment) []string {
	raw := strings.TrimPrefix(c.ID, "gh-review-")
	raw = strings.TrimPrefix(raw, "action-")
	keys := make([]string, 0, 3)
	if raw != "" {
		keys = append(keys, raw)
	}
	if db, ok := githubNodeDatabaseID(raw); ok {
		keys = append(keys, strconv.FormatInt(db, 10))
	}
	if u := strings.TrimSpace(c.URL); u != "" {
		if i := strings.LastIndex(u, "pullrequestreview-"); i >= 0 {
			n := u[i+len("pullrequestreview-"):]
			if j := strings.IndexAny(n, "?#&"); j >= 0 {
				n = n[:j]
			}
			if n != "" {
				keys = append(keys, n)
			}
		}
	}
	return keys
}

func promoteReviewWithKids(c *domain.PRComment, all []domain.PRComment) {
	if c.Kind != "action" {
		return
	}
	oldID := c.ID
	if strings.HasPrefix(oldID, "gh-review-action-") {
		c.ID = "gh-review-" + strings.TrimPrefix(oldID, "gh-review-action-")
		for i := range all {
			if all[i].ParentID == oldID {
				all[i].ParentID = c.ID
			}
		}
	}
	if strings.TrimSpace(c.Verb) == "" {
		c.Verb = reviewActionCardVerb(c.BodyMD)
	}
	c.Kind = "comment"
	c.BodyMD = ""
	c.BodyHTML = ""
}

func reviewActionCardVerb(body string) string {
	switch strings.TrimSpace(body) {
	case "approved":
		return "approved"
	case "requested changes":
		return "requested changes"
	case "dismissed a review":
		return "dismissed a review"
	default:
		return "commented"
	}
}

type ghReviewThread struct {
	ID         string
	Resolved   bool
	CommentIDs []int64
}

func fetchGitHubReviewThreads(ctx context.Context, repoPath, ownerRepo string, prNumber int) ([]ghReviewThread, error) {
	owner, repo, ok := splitOwnerRepo(ownerRepo)
	if !ok || prNumber <= 0 {
		return nil, fmt.Errorf("invalid repo for review threads")
	}
	const q = `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{id isResolved comments(first:50){nodes{databaseId}}}}}}}`
	out, err := runGH(ctx, repoPath, "api", "graphql",
		"-f", "query="+q,
		"-F", "o="+owner,
		"-F", "r="+repo,
		"-F", "n="+strconv.Itoa(prNumber))
	if err != nil {
		return nil, err
	}
	return parseGitHubReviewThreads(out), nil
}

func parseGitHubReviewThreads(raw string) []ghReviewThread {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil
	}
	var payload struct {
		Data struct {
			Repository struct {
				PullRequest struct {
					ReviewThreads struct {
						Nodes []struct {
							ID         string `json:"id"`
							IsResolved bool   `json:"isResolved"`
							Comments   struct {
								Nodes []struct {
									DatabaseID int64 `json:"databaseId"`
								} `json:"nodes"`
							} `json:"comments"`
						} `json:"nodes"`
					} `json:"reviewThreads"`
				} `json:"pullRequest"`
			} `json:"repository"`
		} `json:"data"`
	}
	if json.Unmarshal([]byte(raw), &payload) != nil {
		return nil
	}
	var out []ghReviewThread
	for _, n := range payload.Data.Repository.PullRequest.ReviewThreads.Nodes {
		id := strings.TrimSpace(n.ID)
		if id == "" {
			continue
		}
		t := ghReviewThread{ID: id, Resolved: n.IsResolved}
		for _, c := range n.Comments.Nodes {
			if c.DatabaseID > 0 {
				t.CommentIDs = append(t.CommentIDs, c.DatabaseID)
			}
		}
		out = append(out, t)
	}
	return out
}

func applyReviewThreads(cs []domain.PRComment, threads []ghReviewThread) {
	if len(threads) == 0 {
		return
	}
	byComment := map[string]ghReviewThread{}
	for _, t := range threads {
		for _, id := range t.CommentIDs {
			byComment[fmt.Sprintf("gh-inline-%d", id)] = t
		}
	}
	for i, c := range cs {
		t, ok := byComment[c.ID]
		if !ok {
			continue
		}
		cs[i].ThreadID = t.ID
		cs[i].Resolved = t.Resolved
	}
}

func setGitHubReviewThread(ctx context.Context, repoPath, threadID string, resolved bool) error {
	threadID = strings.TrimSpace(threadID)
	if threadID == "" {
		return fmt.Errorf("empty review thread id")
	}
	mut := "resolveReviewThread"
	if !resolved {
		mut = "unresolveReviewThread"
	}
	query := fmt.Sprintf(
		`mutation($id:ID!){%s(input:{threadId:$id}){thread{isResolved}}}`, mut)
	payload := struct {
		Query     string            `json:"query"`
		Variables map[string]string `json:"variables"`
	}{Query: query, Variables: map[string]string{"id": threadID}}
	raw, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	out, err := runGHWithStdin(ctx, repoPath, string(raw), "api", "graphql", "--input", "-")
	if err != nil {
		return fmt.Errorf("gh api graphql %s: %w", mut, err)
	}
	if strings.Contains(out, `"errors"`) {
		return fmt.Errorf("gh api graphql %s: %s", mut, strings.TrimSpace(out))
	}
	return nil
}

func sortGHComments(cs []domain.PRComment) {
	sort.SliceStable(cs, func(i, j int) bool {
		if cs[i].Kind == "description" && cs[j].Kind != "description" {
			return true
		}
		if cs[j].Kind == "description" && cs[i].Kind != "description" {
			return false
		}
		ti, oi := parseGHTime(cs[i].At)
		tj, oj := parseGHTime(cs[j].At)
		if oi && oj {
			return ti.Before(tj)
		}
		return false
	})
}

func formatGHWhen(raw string, now time.Time) string {
	t, ok := parseGHTime(raw)
	if !ok {
		return "now"
	}
	d := now.Sub(t)
	if d < 0 {
		d = 0
	}
	if d < time.Minute {
		return "now"
	}
	if d < time.Hour {
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	if d < 24*time.Hour {
		return fmt.Sprintf("%dh", int(d.Hours()))
	}
	return fmt.Sprintf("%dd", int(d.Hours()/24))
}

func ownerRepoFromPRURL(url string) (string, bool) {
	u := strings.TrimSpace(url)
	u = strings.TrimSuffix(u, "/")
	for _, p := range []string{"https://", "http://"} {
		u = strings.TrimPrefix(u, p)
	}
	parts := strings.Split(u, "/")
	// github.com/owner/repo/pull/123
	if len(parts) < 3 {
		return "", false
	}
	owner, repo := parts[1], parts[2]
	if owner == "" || repo == "" {
		return "", false
	}
	return owner + "/" + repo, true
}

func mergeGitHubComments(existing, fromGH []domain.PRComment, keepComments, keepReviews bool) []domain.PRComment {
	have := make(map[string]bool, len(fromGH))
	for _, c := range fromGH {
		have[c.ID] = true
	}
	local := make([]domain.PRComment, 0)
	for _, c := range existing {
		if have[c.ID] {
			continue
		}
		if !strings.HasPrefix(c.ID, "gh-") {
			local = append(local, c)
			continue
		}
		if keepComments && strings.HasPrefix(c.ID, "gh-comment-") {
			local = append(local, c)
			continue
		}
		if keepReviews && strings.HasPrefix(c.ID, "gh-review-") {
			local = append(local, c)
			continue
		}
		if keepFreshGitHubInline(c, have) {
			local = append(local, c)
		}
	}
	out := make([]domain.PRComment, 0, len(fromGH)+len(local))
	out = append(out, fromGH...)
	return append(out, local...)
}

// keepFreshGitHubInline holds a just-posted line note until GitHub's list
// endpoint echoes it (the 5s GetPR poll would otherwise drop it).
const droppedCommentTTL = 2 * time.Minute

func droppedCommentKey(prID, id string) string {
	return prID + "\x00" + id
}

func (e *Engine) rememberDroppedComments(prID string, ids []string) {
	if e == nil || prID == "" || len(ids) == 0 {
		return
	}
	e.droppedCommentsMu.Lock()
	defer e.droppedCommentsMu.Unlock()
	if e.droppedComments == nil {
		e.droppedComments = map[string]time.Time{}
	}
	now := time.Now()
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id == "" {
			continue
		}
		e.droppedComments[droppedCommentKey(prID, id)] = now
	}
}

func (e *Engine) forgetDroppedComments(prID string, ids []string) {
	if e == nil || e.droppedComments == nil {
		return
	}
	e.droppedCommentsMu.Lock()
	defer e.droppedCommentsMu.Unlock()
	for _, id := range ids {
		delete(e.droppedComments, droppedCommentKey(prID, id))
	}
}

func (e *Engine) dropRememberedComments(prID string, cs []domain.PRComment) []domain.PRComment {
	if e == nil || len(cs) == 0 {
		return cs
	}
	e.droppedCommentsMu.Lock()
	defer e.droppedCommentsMu.Unlock()
	if len(e.droppedComments) == 0 {
		return cs
	}
	now := time.Now()
	out := make([]domain.PRComment, 0, len(cs))
	for _, c := range cs {
		key := droppedCommentKey(prID, c.ID)
		at, ok := e.droppedComments[key]
		if !ok {
			out = append(out, c)
			continue
		}
		if now.Sub(at) >= droppedCommentTTL {
			delete(e.droppedComments, key)
			out = append(out, c)
		}
	}
	return out
}

type threadResolveHold struct {
	Resolved bool
	At       time.Time
}

func (e *Engine) rememberThreadResolved(prID, threadID string, resolved bool) {
	if e == nil || prID == "" || threadID == "" {
		return
	}
	e.threadResolvedMu.Lock()
	defer e.threadResolvedMu.Unlock()
	if e.threadResolved == nil {
		e.threadResolved = map[string]threadResolveHold{}
	}
	e.threadResolved[droppedCommentKey(prID, threadID)] = threadResolveHold{Resolved: resolved, At: time.Now()}
}

func (e *Engine) forgetThreadResolved(prID, threadID string) {
	if e == nil || e.threadResolved == nil {
		return
	}
	e.threadResolvedMu.Lock()
	defer e.threadResolvedMu.Unlock()
	delete(e.threadResolved, droppedCommentKey(prID, threadID))
}

func (e *Engine) applyRememberedThreadResolved(prID string, cs []domain.PRComment) []domain.PRComment {
	if e == nil || len(cs) == 0 {
		return cs
	}
	e.threadResolvedMu.Lock()
	defer e.threadResolvedMu.Unlock()
	if len(e.threadResolved) == 0 {
		return cs
	}
	now := time.Now()
	holds := map[string]bool{}
	for key, hold := range e.threadResolved {
		if now.Sub(hold.At) >= droppedCommentTTL {
			delete(e.threadResolved, key)
			continue
		}
		_, id, ok := strings.Cut(key, "\x00")
		if !ok || !strings.HasPrefix(key, prID+"\x00") {
			continue
		}
		holds[id] = hold.Resolved
	}
	if len(holds) == 0 {
		return cs
	}
	out := append([]domain.PRComment(nil), cs...)
	for id, resolved := range holds {
		markThreadResolved(out, id, resolved)
	}
	return out
}

func keepFreshGitHubInline(c domain.PRComment, have map[string]bool) bool {
	if have[c.ID] || !strings.HasPrefix(c.ID, "gh-inline-") {
		return false
	}
	if c.Time == "now" {
		return true
	}
	t, ok := parseGHTime(c.At)
	if !ok {
		return false
	}
	return time.Since(t) < 2*time.Minute
}

func enrichGitHubCommits(commits []domain.Commit, authors map[string]ghViewActor, meta map[string]ghCommitMeta) []domain.Commit {
	if len(commits) == 0 {
		return commits
	}
	out := append([]domain.Commit(nil), commits...)
	for i := range out {
		key := strings.TrimSpace(out[i].SHA)
		if key == "" {
			key = strings.TrimSpace(out[i].Hash)
		}
		if a, ok := lookupCommitAuthor(authors, key); ok && a.Login != "" {
			out[i].Author = ghDisplayName(a.Login)
			out[i].AvatarURL = ghAvatarURL(a.Login, a.avatar())
		}
		if cm, ok := lookupCommitMeta(meta, key); ok {
			out[i].Verified = cm.Verified
			out[i].CheckStatus = cm.CheckStatus
			if out[i].SHA == "" {
				out[i].SHA = strings.TrimSpace(key)
			}
		}
	}
	return out
}

func commitsFromGitHubView(rows []ghViewCommit, now time.Time) []domain.Commit {
	if len(rows) == 0 {
		return nil
	}
	out := make([]domain.Commit, 0, len(rows))
	for _, row := range rows {
		oid := strings.TrimSpace(row.OID)
		if oid == "" {
			continue
		}
		author := ""
		for _, a := range row.Authors {
			if login := strings.TrimSpace(a.Login); login != "" {
				author = ghDisplayName(login)
				break
			}
			if name := strings.TrimSpace(a.Name); name != "" {
				author = name
				break
			}
		}
		hash := oid
		if len(hash) > 7 {
			hash = hash[:7]
		}
		c := domain.Commit{
			Hash:    hash,
			SHA:     oid,
			Message: strings.TrimSpace(row.MessageHeadline),
			Body:    strings.TrimSpace(row.MessageBody),
			Author:  author,
			When:    compactAgo(parseGitHubTime(row.CommittedDate), now),
		}
		out = append(out, c)
	}
	return out
}

func commitsEqual(a, b []domain.Commit) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i].Hash != b[i].Hash || a[i].SHA != b[i].SHA || a[i].Author != b[i].Author ||
			a[i].AvatarURL != b[i].AvatarURL || a[i].Verified != b[i].Verified ||
			a[i].CheckStatus != b[i].CheckStatus || a[i].When != b[i].When ||
			a[i].Message != b[i].Message || a[i].Body != b[i].Body {
			return false
		}
	}
	return true
}

func commentsEqual(a, b []domain.PRComment) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i].ID != b[i].ID || a[i].BodyHTML != b[i].BodyHTML || a[i].Author != b[i].Author ||
			a[i].AuthorLogin != b[i].AuthorLogin || a[i].Kind != b[i].Kind || a[i].Verb != b[i].Verb || a[i].BodyMD != b[i].BodyMD ||
			a[i].AvatarURL != b[i].AvatarURL || a[i].Verified != b[i].Verified ||
			a[i].CheckStatus != b[i].CheckStatus || a[i].CommitSHA != b[i].CommitSHA ||
			a[i].URL != b[i].URL || a[i].ParentID != b[i].ParentID || a[i].ReviewID != b[i].ReviewID ||
			a[i].Path != b[i].Path || a[i].Line != b[i].Line || a[i].StartLine != b[i].StartLine ||
			a[i].Side != b[i].Side || a[i].StartSide != b[i].StartSide || a[i].DiffHunk != b[i].DiffHunk ||
			a[i].InReplyToID != b[i].InReplyToID || a[i].ThreadID != b[i].ThreadID || a[i].Resolved != b[i].Resolved ||
			a[i].Outdated != b[i].Outdated ||
			a[i].NodeID != b[i].NodeID || !domain.ReactionsEqual(a[i].Reactions, b[i].Reactions) {
			return false
		}
	}
	return true
}

func githubConversationState(pr domain.PR, snap ghPRSnapshot) (state, group string) {
	if pr.State == "Merged" {
		return pr.State, pr.Group
	}
	switch snap.State {
	case "MERGED":
		return "Merged", prGroupMerged
	case "CLOSED":
		return "Closed", pr.Group
	}
	group = pr.Group
	if group == prGroupMerged {
		group = prGroupOpen
	}
	switch snap.ReviewDecision {
	case "APPROVED":
		return "Approved", group
	case "CHANGES_REQUESTED":
		return "Changes requested", group
	}
	if pr.State == "Approved" || pr.State == "Changes requested" {
		return "In review", group
	}
	if pr.State == "Closed" {
		return "Open", group
	}
	return pr.State, group
}

func postGitHubPRComment(ctx context.Context, repoPath string, number int, file string, line int, body string) error {
	body = strings.TrimSpace(body)
	if body == "" {
		return fmt.Errorf("empty comment")
	}
	if strings.TrimSpace(file) != "" && line > 0 {
		if err := postGitHubInlineComment(ctx, repoPath, number, file, line, body); err == nil {
			return nil
		} else {
			body = fmt.Sprintf("%s:%d\n\n%s", file, line, body)
		}
	}
	_, err := runGH(ctx, repoPath, "pr", "comment", fmt.Sprintf("%d", number), "--body", body)
	if err != nil {
		return fmt.Errorf("gh pr comment %d: %w", number, err)
	}
	return nil
}

func postGitHubInlineComment(ctx context.Context, repoPath string, number int, file string, line int, body string) error {
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", number), "--json", "url,headRefOid")
	if err != nil {
		return err
	}
	var meta struct {
		URL        string `json:"url"`
		HeadRefOid string `json:"headRefOid"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return fmt.Errorf("parse gh pr view: %w", err)
	}
	ownerRepo, ok := ownerRepoFromPRURL(meta.URL)
	if !ok || meta.HeadRefOid == "" {
		return fmt.Errorf("missing repo or head oid for inline comment")
	}
	_, err = runGH(ctx, repoPath, "api",
		fmt.Sprintf("repos/%s/pulls/%d/comments", ownerRepo, number),
		"-f", "body="+body,
		"-f", "commit_id="+meta.HeadRefOid,
		"-f", "path="+file,
		"-F", fmt.Sprintf("line=%d", line),
		"-f", "side=RIGHT")
	return err
}

func submitGitHubPRReview(ctx context.Context, repoPath string, number int, kind, body string, comments []domain.PendingLineComment) error {
	body = strings.TrimSpace(body)
	flag := "--comment"
	event := "COMMENT"
	switch kind {
	case "approve":
		flag = "--approve"
		event = "APPROVE"
		if body == "" {
			body = "Approved"
		}
	case "request":
		flag = "--request-changes"
		event = "REQUEST_CHANGES"
		if body == "" {
			body = "Requested changes"
		}
	default:
		if body == "" {
			body = "Commented"
		}
	}
	if len(comments) > 0 {
		return submitGitHubPRReviewWithComments(ctx, repoPath, number, event, body, comments)
	}
	args := []string{"pr", "review", fmt.Sprintf("%d", number), flag, "--body", body}
	if _, err := runGH(ctx, repoPath, args...); err != nil {
		return fmt.Errorf("gh pr review %d: %w", number, err)
	}
	return nil
}

func submitGitHubPRReviewWithComments(ctx context.Context, repoPath string, number int, event, body string, comments []domain.PendingLineComment) error {
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", number), "--json", "url,headRefOid")
	if err != nil {
		return err
	}
	var meta struct {
		URL        string `json:"url"`
		HeadRefOid string `json:"headRefOid"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return fmt.Errorf("parse gh pr view: %w", err)
	}
	ownerRepo, ok := ownerRepoFromPRURL(meta.URL)
	if !ok || meta.HeadRefOid == "" {
		return fmt.Errorf("missing repo or head oid for review")
	}
	type reviewComment struct {
		Path      string `json:"path"`
		Line      int    `json:"line"`
		Side      string `json:"side"`
		Body      string `json:"body"`
		StartLine int    `json:"start_line,omitempty"`
		StartSide string `json:"start_side,omitempty"`
	}
	payload := struct {
		CommitID string          `json:"commit_id"`
		Body     string          `json:"body"`
		Event    string          `json:"event"`
		Comments []reviewComment `json:"comments"`
	}{CommitID: meta.HeadRefOid, Body: body, Event: event}
	for _, pc := range comments {
		side := pendingSide(pc.Side)
		if pc.Line <= 0 || strings.TrimSpace(pc.File) == "" || strings.TrimSpace(pc.Body) == "" {
			continue
		}
		rc := reviewComment{
			Path: pc.File, Line: pc.Line, Side: side, Body: pc.Body,
		}
		if pc.StartLine > 0 && pc.StartLine < pc.Line {
			rc.StartLine = pc.StartLine
			startSide := pendingSide(pc.StartSide)
			if pc.StartSide == "" {
				startSide = side
			}
			rc.StartSide = startSide
		}
		payload.Comments = append(payload.Comments, rc)
	}
	raw, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	if _, err := runGHWithStdin(ctx, repoPath, string(raw), "api",
		fmt.Sprintf("repos/%s/pulls/%d/reviews", ownerRepo, number),
		"--method", "POST", "--input", "-"); err != nil {
		return fmt.Errorf("gh api pull review %d: %w", number, err)
	}
	return nil
}

type ghRepoPermissions struct {
	Admin    bool `json:"admin"`
	Maintain bool `json:"maintain"`
	Push     bool `json:"push"`
}

type ghRepoSettingsJSON struct {
	Permissions      ghRepoPermissions `json:"permissions"`
	AllowMergeCommit *bool             `json:"allow_merge_commit"`
	AllowSquashMerge *bool             `json:"allow_squash_merge"`
	AllowRebaseMerge *bool             `json:"allow_rebase_merge"`
}

type ghViewer struct {
	Login               string
	CanWrite            bool
	AllowedMergeMethods []string
}

func fetchGitHubViewer(ctx context.Context, repoPath, ownerRepo string) ghViewer {
	var loginOut, repoOut string
	var loginErr, repoErr error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		loginOut, loginErr = runGH(ctx, repoPath, "api", "user", "-q", ".login")
	}()
	go func() {
		defer wg.Done()
		repoOut, repoErr = runGH(ctx, repoPath, "api", "repos/"+ownerRepo, "-q",
			"{permissions: .permissions, allow_merge_commit: .allow_merge_commit, allow_squash_merge: .allow_squash_merge, allow_rebase_merge: .allow_rebase_merge}")
	}()
	wg.Wait()
	if loginErr != nil {
		return ghViewer{}
	}
	v := ghViewer{Login: strings.TrimSpace(loginOut)}
	if repoErr != nil {
		return v
	}
	settings, ok := parseGitHubRepoSettings(repoOut)
	if !ok {
		return v
	}
	v.CanWrite = settings.Permissions.Admin || settings.Permissions.Maintain || settings.Permissions.Push
	v.AllowedMergeMethods = allowedMergeMethodsFromRepo(settings)
	return v
}

func parseGitHubRepoSettings(raw string) (ghRepoSettingsJSON, bool) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ghRepoSettingsJSON{}, false
	}
	var settings ghRepoSettingsJSON
	if json.Unmarshal([]byte(raw), &settings) != nil {
		return ghRepoSettingsJSON{}, false
	}
	return settings, true
}

func allowedMergeMethodsFromRepo(r ghRepoSettingsJSON) []string {
	if r.AllowMergeCommit == nil && r.AllowSquashMerge == nil && r.AllowRebaseMerge == nil {
		return nil
	}
	allow := func(p *bool) bool { return p == nil || *p }
	var out []string
	if allow(r.AllowMergeCommit) {
		out = append(out, string(domain.MergeMethodMerge))
	}
	if allow(r.AllowSquashMerge) {
		out = append(out, string(domain.MergeMethodSquash))
	}
	if allow(r.AllowRebaseMerge) {
		out = append(out, string(domain.MergeMethodRebase))
	}
	return out
}

func setGitHubPRDraft(ctx context.Context, repoPath string, number int, draft bool) error {
	if number <= 0 {
		return fmt.Errorf("invalid GitHub PR number %d", number)
	}
	args := []string{"pr", "ready", fmt.Sprintf("%d", number)}
	if draft {
		args = append(args, "--undo")
	}
	if _, err := runGH(ctx, repoPath, args...); err != nil {
		if draft {
			return fmt.Errorf("gh pr ready --undo %d: %w", number, err)
		}
		return fmt.Errorf("gh pr ready %d: %w", number, err)
	}
	return nil
}

func editGitHubPR(ctx context.Context, repoPath string, number int, title, body string) error {
	title = strings.TrimSpace(title)
	if title == "" {
		return fmt.Errorf("empty title")
	}
	args := []string{"pr", "edit", fmt.Sprintf("%d", number), "--title", title}
	if body != omitPREditBody {
		args = append(args, "--body", body)
	}
	if _, err := runGH(ctx, repoPath, args...); err != nil {
		return fmt.Errorf("gh pr edit %d: %w", number, err)
	}
	return nil
}

func editGitHubPRComment(ctx context.Context, repoPath string, number int, commentID, body string) error {
	body = strings.TrimSpace(body)
	if body == "" {
		return fmt.Errorf("empty comment")
	}
	commentID = strings.TrimSpace(commentID)
	switch {
	case commentID == "gh-description":
		return editGitHubPRBody(ctx, repoPath, number, body)
	case strings.HasPrefix(commentID, "gh-comment-"):
		return editGitHubGraphQLComment(ctx, repoPath, "updateIssueComment", "id",
			strings.TrimPrefix(commentID, "gh-comment-"), body)
	case strings.HasPrefix(commentID, "gh-review-"):
		return editGitHubGraphQLComment(ctx, repoPath, "updatePullRequestReview", "pullRequestReviewId",
			strings.TrimPrefix(commentID, "gh-review-"), body)
	case strings.HasPrefix(commentID, "gh-inline-"):
		return editGitHubInlineComment(ctx, repoPath, number, strings.TrimPrefix(commentID, "gh-inline-"), body)
	default:
		return fmt.Errorf("unsupported comment id %s", commentID)
	}
}

func editGitHubPRBody(ctx context.Context, repoPath string, number int, body string) error {
	if _, err := runGH(ctx, repoPath, "pr", "edit", fmt.Sprintf("%d", number), "--body", body); err != nil {
		return fmt.Errorf("gh pr edit %d body: %w", number, err)
	}
	return nil
}

func editGitHubGraphQLComment(ctx context.Context, repoPath, mutation, idField, nodeID, body string) error {
	nodeID = strings.TrimSpace(nodeID)
	if nodeID == "" {
		return fmt.Errorf("empty github comment id")
	}
	query := fmt.Sprintf(
		`mutation($id:ID!,$body:String!){%s(input:{%s:$id,body:$body}){clientMutationId}}`,
		mutation, idField)
	payload := struct {
		Query     string            `json:"query"`
		Variables map[string]string `json:"variables"`
	}{Query: query, Variables: map[string]string{"id": nodeID, "body": body}}
	raw, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	out, err := runGHWithStdin(ctx, repoPath, string(raw), "api", "graphql", "--input", "-")
	if err != nil {
		return fmt.Errorf("gh api graphql %s: %w", mutation, err)
	}
	if strings.Contains(out, `"errors"`) {
		return fmt.Errorf("gh api graphql %s: %s", mutation, strings.TrimSpace(out))
	}
	return nil
}

func editGitHubInlineComment(ctx context.Context, repoPath string, number int, restID, body string) error {
	if _, err := strconv.ParseInt(restID, 10, 64); err != nil {
		return fmt.Errorf("invalid inline comment id %q", restID)
	}
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", number), "--json", "url")
	if err != nil {
		return err
	}
	var meta struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return fmt.Errorf("parse gh pr view: %w", err)
	}
	ownerRepo, ok := ownerRepoFromPRURL(meta.URL)
	if !ok {
		return fmt.Errorf("missing repo for inline comment edit")
	}
	if _, err := runGH(ctx, repoPath, "api",
		fmt.Sprintf("repos/%s/pulls/comments/%s", ownerRepo, restID),
		"-X", "PATCH", "-f", "body="+body); err != nil {
		return fmt.Errorf("gh api pull comment %s: %w", restID, err)
	}
	return nil
}

func deleteGitHubPRComment(ctx context.Context, repoPath string, number int, commentID string) error {
	commentID = strings.TrimSpace(commentID)
	switch {
	case strings.HasPrefix(commentID, "gh-comment-"):
		return deleteGitHubGraphQLComment(ctx, repoPath, "deleteIssueComment",
			strings.TrimPrefix(commentID, "gh-comment-"))
	case strings.HasPrefix(commentID, "gh-inline-"):
		return deleteGitHubInlineComment(ctx, repoPath, number, commentID)
	default:
		return fmt.Errorf("unsupported comment id %s", commentID)
	}
}

func deleteGitHubGraphQLComment(ctx context.Context, repoPath, mutation, nodeID string) error {
	nodeID = strings.TrimSpace(nodeID)
	if nodeID == "" {
		return fmt.Errorf("empty github comment id")
	}
	query := fmt.Sprintf(`mutation($id:ID!){%s(input:{id:$id}){clientMutationId}}`, mutation)
	payload := struct {
		Query     string            `json:"query"`
		Variables map[string]string `json:"variables"`
	}{Query: query, Variables: map[string]string{"id": nodeID}}
	raw, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	out, err := runGHWithStdin(ctx, repoPath, string(raw), "api", "graphql", "--input", "-")
	if err != nil {
		return fmt.Errorf("gh api graphql %s: %w", mutation, err)
	}
	if strings.Contains(out, `"errors"`) {
		return fmt.Errorf("gh api graphql %s: %s", mutation, strings.TrimSpace(out))
	}
	return nil
}

func deleteGitHubInlineComment(ctx context.Context, repoPath string, number int, commentID string) error {
	restID := strings.TrimPrefix(strings.TrimSpace(commentID), "gh-inline-")
	if _, err := strconv.ParseInt(restID, 10, 64); err != nil {
		return fmt.Errorf("invalid inline comment id %q", commentID)
	}
	ownerRepo, err := ownerRepoForPR(ctx, repoPath, number)
	if err != nil {
		return err
	}
	if _, err := runGH(ctx, repoPath, "api",
		fmt.Sprintf("repos/%s/pulls/comments/%s", ownerRepo, restID),
		"-X", "DELETE"); err != nil {
		return fmt.Errorf("gh api delete pull comment %s: %w", restID, err)
	}
	return nil
}

func replyGitHubPRComment(ctx context.Context, repoPath string, number int, commentID, body string) (string, error) {
	body = strings.TrimSpace(body)
	if body == "" {
		return "", fmt.Errorf("empty comment")
	}
	restID := strings.TrimPrefix(strings.TrimSpace(commentID), "gh-inline-")
	if _, err := strconv.ParseInt(restID, 10, 64); err != nil {
		return "", fmt.Errorf("invalid inline comment id %q", commentID)
	}
	ownerRepo, err := ownerRepoForPR(ctx, repoPath, number)
	if err != nil {
		return "", err
	}
	out, err := runGH(ctx, repoPath, "api",
		fmt.Sprintf("repos/%s/pulls/%d/comments", ownerRepo, number),
		"-f", "body="+body,
		"-F", "in_reply_to="+restID)
	if err != nil {
		return "", fmt.Errorf("gh api reply pull comment %s: %w", restID, err)
	}
	return out, nil
}

func parsePostedInlineComment(raw string, now time.Time, prURL string) (domain.PRComment, bool) {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw[0] != '{' {
		return domain.PRComment{}, false
	}
	cs := parseGitHubInlineComments("["+raw+"]", now, prURL)
	if len(cs) != 1 || strings.TrimSpace(cs[0].ID) == "" {
		return domain.PRComment{}, false
	}
	return cs[0], true
}

func ownerRepoForPR(ctx context.Context, repoPath string, number int) (string, error) {
	out, err := runGH(ctx, repoPath, "pr", "view", fmt.Sprintf("%d", number), "--json", "url")
	if err != nil {
		return "", err
	}
	var meta struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &meta); err != nil {
		return "", fmt.Errorf("parse gh pr view: %w", err)
	}
	ownerRepo, ok := ownerRepoFromPRURL(meta.URL)
	if !ok {
		return "", fmt.Errorf("missing repo for pull comment")
	}
	return ownerRepo, nil
}

// prConversationMinInterval is how often GetPR may start a GitHub conversation
// snapshot for the same PR. Shorter than this and overlapping GetPR calls
// (the live-checks poll, pr.updated refetch) coalesce onto one in-flight fetch.
// 20s matches the open-PR poll closely enough that the view you are looking
// at does not sit a minute stale, without spawning a snapshot on every tick.
const prConversationMinInterval = 20 * time.Second

// ScheduleRefreshPRConversation runs RefreshPRConversation in the background
// at most once per prConversationMinInterval per PR (single-flight). GetPR
// uses this so the live-checks poll cannot storm GitHub with a gh snapshot.
func (e *Engine) ScheduleRefreshPRConversation(prID string) {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub || pr.Number <= 0 {
		return
	}
	if t, ok := e.prConvLast.Load(prID); ok {
		if time.Since(t.(time.Time)) < prConversationMinInterval {
			return
		}
	}
	if _, loaded := e.prConvBusy.LoadOrStore(prID, struct{}{}); loaded {
		return
	}
	go func() {
		defer e.prConvBusy.Delete(prID)
		changed := e.RefreshPRConversation(prID)
		e.prConvLast.Store(prID, time.Now())
		if !changed {
			return
		}
		e.publishPRUpdated(prID)
	}()
}

// RefreshPRConversation replaces a GitHub PR's thread + review state with the
// live GitHub conversation. Recur-only comments (agent chatter, no gh- id) are
// kept. Fetch errors leave the persisted thread alone. Reports whether the
// stored PR changed so callers can publish pr.updated.
func (e *Engine) RefreshPRConversation(prID string) bool {
	pr, ok := e.Store.PR(prID)
	if !ok || pr.Source != domain.SourceGitHub || pr.Number <= 0 {
		return false
	}
	repoPath := e.prRepoPath(pr)
	if repoPath == "" {
		return false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	snap, err := getGitHubPRSnapshot(ctx, repoPath, pr.Number)
	if err != nil {
		return false
	}
	comments := e.applyRememberedCommentReactions(prID, e.applyRememberedThreadResolved(prID, e.dropRememberedComments(prID, mergeGitHubComments(pr.Comments, snap.Comments, snap.commentsTruncated, snap.reviewsTruncated))))
	commits := enrichGitHubCommits(pr.Commits, snap.commitAuthors, snap.commitMeta)
	if len(commits) == 0 {
		commits = enrichGitHubCommits(commitsFromGitHubView(snap.viewCommits, time.Now()), snap.commitAuthors, snap.commitMeta)
	}
	applyGHCommitAvatars(commits, snap.authorAvatars)
	state, group := githubConversationState(pr, snap)
	reasons, reviewReq, group := conversationAttention(pr, snap, state, group)
	people := mergeParticipants(pr.Participants, snapshotParticipants(snap, comments))
	reviewers := snapshotReviewers(snap, comments)
	title := pr.Title
	if snap.Title != "" {
		title = snap.Title
	}
	createdAt := pr.CreatedAt
	if c := normalizePRCreatedAt(snap.CreatedAt); c != "" {
		createdAt = c
	}
	updatedAt := pr.UpdatedAt
	if u := normalizePRCreatedAt(snap.UpdatedAt); u != "" {
		updatedAt = u
	}
	url := strings.TrimSpace(snap.URL)
	authorLogin := strings.TrimSpace(snap.AuthorLogin)
	viewerLogin := strings.TrimSpace(snap.ViewerLogin)
	headSHA := strings.TrimSpace(snap.HeadRefOid)
	if commentsEqual(pr.Comments, comments) && commitsEqual(pr.Commits, commits) && pr.State == state && pr.Group == group && pr.Title == title && (url == "" || pr.URL == url) && (headSHA == "" || pr.HeadSHA == headSHA) &&
		pr.AuthorLogin == authorLogin && pr.ViewerLogin == viewerLogin && pr.ViewerCanWrite == snap.ViewerCanWrite &&
		(snap.NodeID == "" || pr.NodeID == snap.NodeID) &&
		pr.CreatedAt == createdAt && pr.UpdatedAt == updatedAt &&
		pr.Mergeable == snap.Mergeable && pr.MergeStateStatus == snap.MergeStateStatus &&
		pr.ReviewDecision == snap.ReviewDecision && pr.Draft == snap.IsDraft &&
		pr.ReviewRequested == reviewReq && slices.Equal(pr.AttentionReasons, reasons) &&
		slices.Equal(pr.Participants, people) && slices.Equal(pr.Reviewers, reviewers) &&
		slices.Equal(pr.AllowedMergeMethods, snap.AllowedMergeMethods) {
		return false
	}
	becameMerged := state == "Merged" && pr.State != "Merged"
	e.Store.UpdatePR(prID, func(p *domain.PR) {
		p.Comments = comments
		p.Commits = commits
		p.Title = title
		if url != "" {
			p.URL = url
		}
		if authorLogin != "" {
			p.AuthorLogin = authorLogin
		}
		if viewerLogin != "" {
			p.ViewerLogin = viewerLogin
		}
		if headSHA != "" {
			p.HeadSHA = headSHA
		}
		p.ViewerCanWrite = snap.ViewerCanWrite
		if snap.NodeID != "" {
			p.NodeID = snap.NodeID
		}
		p.Mergeable = snap.Mergeable
		p.MergeStateStatus = snap.MergeStateStatus
		p.ReviewDecision = snap.ReviewDecision
		p.ReviewRequested = reviewReq
		p.AttentionReasons = reasons
		p.Participants = people
		p.Reviewers = reviewers
		p.Draft = snap.IsDraft
		p.AllowedMergeMethods = snap.AllowedMergeMethods
		if createdAt != "" {
			p.CreatedAt = createdAt
		}
		if updatedAt != "" {
			p.UpdatedAt = updatedAt
		}
		// Recompute from the live row: a merge can land while this snapshot
		// was in flight, and the stale `state/group` above would un-merge it.
		liveState, liveGroup := githubConversationState(*p, snap)
		_, _, liveGroup = conversationAttention(*p, snap, liveState, liveGroup)
		p.State = liveState
		p.Group = liveGroup
	})
	if becameMerged {
		e.markMerged(prID, updatedAt)
	}
	return true
}
