package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
)

// Combined GraphQL snapshot for PR view + threads + rollup + reactions + viewer.
// Timeline stays on REST: GraphQL timelineItems drops Copilot work events
// (TestLiveGitHubGraphQLOmitsCopilotWorkEvents).
const githubPRSnapshotGraphQL = `query($o:String!,$r:String!,$n:Int!){
  rateLimit { cost remaining nodeCount }
  viewer { login }
  repository(owner:$o,name:$r){
    viewerPermission
    mergeCommitAllowed
    squashMergeAllowed
    rebaseMergeAllowed
    pullRequest(number:$n){
      id title body state reviewDecision mergeable mergeStateStatus isDraft
      url headRefOid headRefName baseRefName createdAt updatedAt
      author { login avatarUrl }
      reactionGroups { content viewerHasReacted reactors { totalCount } }
      reviewRequests(first:20){ nodes { requestedReviewer { __typename ... on User { login avatarUrl } ... on Bot { login avatarUrl } ... on Team { name } } } }
      assignees(first:20){ nodes { login avatarUrl } }
      comments(last:100){ pageInfo { hasPreviousPage } nodes { id databaseId url author { login avatarUrl } body createdAt reactionGroups { content viewerHasReacted reactors { totalCount } } } }
      reviews(last:100){ pageInfo { hasPreviousPage } nodes { id databaseId url author { login avatarUrl } body state submittedAt reactionGroups { content viewerHasReacted reactors { totalCount } } } }
      commits(last:100){ nodes { commit { oid url messageHeadline messageBody committedDate signature { isValid } statusCheckRollup { state } authors(first:8){ nodes { name user { login avatarUrl } } } } } }
      reviewThreads(first:100){ nodes { id isResolved diffSide startDiffSide comments(first:50){ nodes { id databaseId url author { login avatarUrl } body createdAt path originalLine line originalStartLine startLine outdated diffHunk replyTo { databaseId } pullRequestReview { databaseId } reactionGroups { content viewerHasReacted reactors { totalCount } } } } } }
    }
  }
}`

func githubPRSnapshotGraphQLQuery(includeCheckRollup bool) string {
	if includeCheckRollup {
		return githubPRSnapshotGraphQL
	}
	// statusCheckRollup needs checks/statuses permission; omit so a PAT/App
	// that can read the PR still gets title, comments, reviews, and threads.
	return strings.Replace(githubPRSnapshotGraphQL, "statusCheckRollup { state }", "", 1)
}

func fetchGitHubPRSnapshotGraphQL(ctx context.Context, repoPath, owner, repo string, prNumber int) (string, error) {
	run := func(q string) (string, error) {
		return runGH(ctx, repoPath, "api", "graphql",
			"-f", "query="+q,
			"-F", "o="+owner,
			"-F", "r="+repo,
			"-F", "n="+strconv.Itoa(prNumber))
	}
	out, err := run(githubPRSnapshotGraphQLQuery(true))
	if err == nil {
		return out, nil
	}
	out2, err2 := run(githubPRSnapshotGraphQLQuery(false))
	if err2 != nil {
		return "", err
	}
	return out2, nil
}

type gqlPRSnapshotEnvelope struct {
	Data struct {
		Viewer struct {
			Login string `json:"login"`
		} `json:"viewer"`
		Repository struct {
			ViewerPermission   string        `json:"viewerPermission"`
			MergeCommitAllowed bool          `json:"mergeCommitAllowed"`
			SquashMergeAllowed bool          `json:"squashMergeAllowed"`
			RebaseMergeAllowed bool          `json:"rebaseMergeAllowed"`
			PullRequest        gqlSnapshotPR `json:"pullRequest"`
		} `json:"repository"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type gqlSnapshotPR struct {
	ID               string      `json:"id"`
	Title            string      `json:"title"`
	Body             string      `json:"body"`
	State            string      `json:"state"`
	ReviewDecision   string      `json:"reviewDecision"`
	Mergeable        string      `json:"mergeable"`
	MergeStateStatus string      `json:"mergeStateStatus"`
	IsDraft          bool        `json:"isDraft"`
	HeadRefOid       string      `json:"headRefOid"`
	HeadRefName      string      `json:"headRefName"`
	BaseRefName      string      `json:"baseRefName"`
	URL              string      `json:"url"`
	CreatedAt        string      `json:"createdAt"`
	UpdatedAt        string      `json:"updatedAt"`
	Author           ghViewActor `json:"author"`
	ReviewRequests   struct {
		Nodes []struct {
			RequestedReviewer *gqlRequestedReviewer `json:"requestedReviewer"`
		} `json:"nodes"`
	} `json:"reviewRequests"`
	Assignees struct {
		Nodes []ghListedActor `json:"nodes"`
	} `json:"assignees"`
	Comments struct {
		PageInfo ghGQLPageInfo        `json:"pageInfo"`
		Nodes    []gqlViewCommentNode `json:"nodes"`
	} `json:"comments"`
	Reviews struct {
		PageInfo ghGQLPageInfo       `json:"pageInfo"`
		Nodes    []gqlViewReviewNode `json:"nodes"`
	} `json:"reviews"`
	Commits struct {
		Nodes []gqlCommitEdge `json:"nodes"`
	} `json:"commits"`
	ReviewThreads struct {
		Nodes []gqlReviewThreadNode `json:"nodes"`
	} `json:"reviewThreads"`
}

type gqlRequestedReviewer struct {
	Typename  string `json:"__typename"`
	Login     string `json:"login"`
	AvatarURL string `json:"avatarUrl"`
	Name      string `json:"name"`
}

type gqlViewCommentNode struct {
	ID        string      `json:"id"`
	URL       string      `json:"url"`
	Author    ghViewActor `json:"author"`
	Body      string      `json:"body"`
	CreatedAt string      `json:"createdAt"`
}

type gqlViewReviewNode struct {
	ID          string      `json:"id"`
	URL         string      `json:"url"`
	Author      ghViewActor `json:"author"`
	Body        string      `json:"body"`
	State       string      `json:"state"`
	SubmittedAt string      `json:"submittedAt"`
}

type gqlCommitEdge struct {
	Commit struct {
		OID             string `json:"oid"`
		MessageHeadline string `json:"messageHeadline"`
		MessageBody     string `json:"messageBody"`
		CommittedDate   string `json:"committedDate"`
		Authors         struct {
			Nodes []struct {
				Name string `json:"name"`
				User *struct {
					Login string `json:"login"`
				} `json:"user"`
			} `json:"nodes"`
		} `json:"authors"`
	} `json:"commit"`
}

type gqlReviewThreadNode struct {
	ID            string `json:"id"`
	DiffSide      string `json:"diffSide"`
	StartDiffSide string `json:"startDiffSide"`
	Comments      struct {
		Nodes []gqlReviewCommentNode `json:"nodes"`
	} `json:"comments"`
}

type gqlReviewCommentNode struct {
	DatabaseID        int64       `json:"databaseId"`
	URL               string      `json:"url"`
	Author            ghViewActor `json:"author"`
	Body              string      `json:"body"`
	CreatedAt         string      `json:"createdAt"`
	Path              string      `json:"path"`
	OriginalLine      int         `json:"originalLine"`
	Line              *int        `json:"line"`
	OriginalStartLine int         `json:"originalStartLine"`
	StartLine         *int        `json:"startLine"`
	DiffHunk          string      `json:"diffHunk"`
	Outdated          bool        `json:"outdated"`
	ReplyTo           *struct {
		DatabaseID int64 `json:"databaseId"`
	} `json:"replyTo"`
	PullRequestReview *struct {
		DatabaseID int64 `json:"databaseId"`
	} `json:"pullRequestReview"`
}

// fetchGitHubPRSnapshot loads a PR with one GraphQL query plus REST timeline.
// REST keeps Copilot work events that GraphQL timelineItems hides.
func fetchGitHubPRSnapshot(ctx context.Context, repoPath string, prNumber int) (ghPRSnapshot, error) {
	if prNumber <= 0 {
		return ghPRSnapshot{}, fmt.Errorf("invalid GitHub PR number %d", prNumber)
	}
	owner, repo, err := githubRepoOwnerName(ctx, repoPath)
	if err != nil {
		return ghPRSnapshot{}, err
	}
	var gqlOut, tlOut string
	var gqlErr, tlErr error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		gqlOut, gqlErr = fetchGitHubPRSnapshotGraphQL(ctx, repoPath, owner, repo, prNumber)
	}()
	go func() {
		defer wg.Done()
		tlOut, tlErr = runGH(ctx, repoPath, githubPRTimelineAPIArgs(owner, repo, prNumber)...)
	}()
	wg.Wait()
	if gqlErr != nil {
		return ghPRSnapshot{}, gqlErr
	}
	now := time.Now()
	snap, err := parseGitHubPRSnapshotGraphQL(gqlOut, now)
	if err != nil {
		return ghPRSnapshot{}, err
	}
	if tlErr == nil {
		return finishGitHubPRSnapshot(snap, flattenJSONArrayPages(tlOut), now), nil
	}
	return finishGitHubPRSnapshot(snap, "", now), nil
}

func githubPRTimelineAPIArgs(owner, repo string, prNumber int) []string {
	return []string{
		"api", "--paginate", "--slurp",
		fmt.Sprintf("repos/%s/%s/issues/%d/timeline?per_page=100", owner, repo, prNumber),
		"-H", "Accept: application/vnd.github+json",
	}
}

// flattenJSONArrayPages turns `gh api --paginate --slurp` output (an array of
// page arrays) into one JSON array. A flat array is returned as-is.
func flattenJSONArrayPages(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return raw
	}
	var pages [][]json.RawMessage
	if json.Unmarshal([]byte(raw), &pages) != nil {
		return raw
	}
	n := 0
	for _, p := range pages {
		n += len(p)
	}
	all := make([]json.RawMessage, 0, n)
	for _, p := range pages {
		all = append(all, p...)
	}
	b, err := json.Marshal(all)
	if err != nil {
		return raw
	}
	return string(b)
}

func parseGitHubPRSnapshotGraphQL(raw string, now time.Time) (ghPRSnapshot, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ghPRSnapshot{}, fmt.Errorf("empty pull request graphql")
	}
	var env gqlPRSnapshotEnvelope
	if err := json.Unmarshal([]byte(raw), &env); err != nil {
		return ghPRSnapshot{}, fmt.Errorf("parse pull request graphql: %w", err)
	}
	pr := env.Data.Repository.PullRequest
	if strings.TrimSpace(pr.ID) == "" && strings.TrimSpace(pr.URL) == "" {
		if len(env.Errors) > 0 && strings.TrimSpace(env.Errors[0].Message) != "" {
			return ghPRSnapshot{}, fmt.Errorf("pull request graphql: %s", env.Errors[0].Message)
		}
		return ghPRSnapshot{}, fmt.Errorf("empty pull request graphql")
	}
	snap := parseGitHubPRViewJSON(graphQLToPRView(pr), now)
	snap.commentsTruncated = pr.Comments.PageInfo.HasPreviousPage
	snap.reviewsTruncated = pr.Reviews.PageInfo.HasPreviousPage
	snap.Comments = append(snap.Comments, commentsFromInlineRows(inlineCommentsFromGraphQL(pr), now, snap.URL)...)
	applyReviewThreads(snap.Comments, parseGitHubReviewThreads(raw))
	meta, avatars := parseGitHubCommitRollup(raw)
	snap.commitMeta = meta
	snap.authorAvatars = avatars
	hits, nodeID, rerr := parseGitHubReactions(raw)
	if rerr == nil {
		applyGitHubReactions(snap.Comments, hits)
		if nodeID != "" {
			snap.NodeID = nodeID
		}
	}
	viewer := githubViewerFromSnapshot(
		env.Data.Viewer.Login,
		env.Data.Repository.ViewerPermission,
		env.Data.Repository.MergeCommitAllowed,
		env.Data.Repository.SquashMergeAllowed,
		env.Data.Repository.RebaseMergeAllowed,
	)
	snap.ViewerLogin = viewer.Login
	snap.ViewerCanWrite = viewer.CanWrite
	snap.AllowedMergeMethods = viewer.AllowedMergeMethods
	return snap, nil
}

func graphQLToPRView(pr gqlSnapshotPR) ghPRViewJSON {
	view := ghPRViewJSON{
		ID:               pr.ID,
		Title:            pr.Title,
		Body:             pr.Body,
		State:            pr.State,
		ReviewDecision:   pr.ReviewDecision,
		Mergeable:        pr.Mergeable,
		MergeStateStatus: pr.MergeStateStatus,
		IsDraft:          pr.IsDraft,
		HeadRefOid:       pr.HeadRefOid,
		HeadRefName:      pr.HeadRefName,
		BaseRefName:      pr.BaseRefName,
		URL:              pr.URL,
		CreatedAt:        pr.CreatedAt,
		UpdatedAt:        pr.UpdatedAt,
		Author:           pr.Author,
		Assignees:        pr.Assignees.Nodes,
	}
	for _, n := range pr.Comments.Nodes {
		view.Comments = append(view.Comments, ghViewComment(n))
	}
	for _, n := range pr.Reviews.Nodes {
		view.Reviews = append(view.Reviews, ghViewReview(n))
	}
	for _, n := range pr.Commits.Nodes {
		var authors []ghViewCommitAuthor
		for _, a := range n.Commit.Authors.Nodes {
			login := ""
			if a.User != nil {
				login = a.User.Login
			}
			authors = append(authors, ghViewCommitAuthor{Login: login, Name: a.Name})
		}
		view.Commits = append(view.Commits, ghViewCommit{
			OID:             n.Commit.OID,
			MessageHeadline: n.Commit.MessageHeadline,
			MessageBody:     n.Commit.MessageBody,
			CommittedDate:   n.Commit.CommittedDate,
			Authors:         authors,
		})
	}
	for _, n := range pr.ReviewRequests.Nodes {
		if n.RequestedReviewer == nil {
			continue
		}
		login := strings.TrimSpace(n.RequestedReviewer.Login)
		if login == "" {
			login = strings.TrimSpace(n.RequestedReviewer.Name)
		}
		if login == "" {
			continue
		}
		view.ReviewRequests = append(view.ReviewRequests, ghListedActor{
			Login:     login,
			AvatarURL: n.RequestedReviewer.AvatarURL,
		})
	}
	return view
}

func inlineCommentsFromGraphQL(pr gqlSnapshotPR) []ghInlineComment {
	var rows []ghInlineComment
	for _, th := range pr.ReviewThreads.Nodes {
		for _, n := range th.Comments.Nodes {
			if n.DatabaseID <= 0 {
				continue
			}
			row := ghInlineComment{
				ID:                n.DatabaseID,
				HTMLURL:           n.URL,
				Body:              n.Body,
				Path:              n.Path,
				Line:              n.Line,
				OriginalLine:      n.OriginalLine,
				StartLine:         n.StartLine,
				OriginalStartLine: n.OriginalStartLine,
				Side:              th.DiffSide,
				StartSide:         th.StartDiffSide,
				DiffHunk:          n.DiffHunk,
				CreatedAt:         n.CreatedAt,
				User:              n.Author,
			}
			if n.Outdated {
				row.Line = nil
			}
			if n.ReplyTo != nil {
				row.InReplyToID = n.ReplyTo.DatabaseID
			}
			if n.PullRequestReview != nil {
				row.PullRequestReviewID = n.PullRequestReview.DatabaseID
			}
			rows = append(rows, row)
		}
	}
	return rows
}

func githubViewerFromSnapshot(login, perm string, merge, squash, rebase bool) ghViewer {
	return ghViewer{
		Login:               strings.TrimSpace(login),
		CanWrite:            githubPermissionCanWrite(perm),
		AllowedMergeMethods: allowedMergeMethodsFromFlags(merge, squash, rebase),
	}
}

func githubPermissionCanWrite(perm string) bool {
	switch strings.ToUpper(strings.TrimSpace(perm)) {
	case "WRITE", "MAINTAIN", "ADMIN":
		return true
	default:
		return false
	}
}

func allowedMergeMethodsFromFlags(merge, squash, rebase bool) []string {
	var out []string
	if merge {
		out = append(out, string(domain.MergeMethodMerge))
	}
	if squash {
		out = append(out, string(domain.MergeMethodSquash))
	}
	if rebase {
		out = append(out, string(domain.MergeMethodRebase))
	}
	return out
}

func finishGitHubPRSnapshot(snap ghPRSnapshot, timelineRaw string, now time.Time) ghPRSnapshot {
	if strings.TrimSpace(timelineRaw) != "" {
		snap.Comments = append(snap.Comments, parseGitHubTimeline(timelineRaw, now, snap.commitAuthors, snap.commitMeta, snap.URL, snap.HeadRefName, snap.BaseRefName)...)
	}
	attachReviewComments(snap.Comments)
	avatars := snap.authorAvatars
	if avatars == nil {
		avatars = map[string]string{}
	}
	for _, c := range snap.Comments {
		if strings.Contains(c.AvatarURL, "avatars.githubusercontent.com/") {
			login := c.AuthorLogin
			if login == "" {
				login = c.Author
			}
			rememberGHAvatar(avatars, ghViewActor{Login: login, AvatarURL: c.AvatarURL})
		}
	}
	if len(avatars) == 0 {
		avatars = nil
	}
	snap.authorAvatars = avatars
	applyGHAvatars(snap.Comments, avatars)
	sortGHComments(snap.Comments)
	return snap
}
