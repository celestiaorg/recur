package engine

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestParseGitHubPRSnapshotGraphQL(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	gql := `{
	  "data": {
	    "viewer": {"login": "loopbot"},
	    "repository": {
	      "viewerPermission": "WRITE",
	      "mergeCommitAllowed": false,
	      "squashMergeAllowed": true,
	      "rebaseMergeAllowed": false,
	      "pullRequest": {
	        "id": "PR_1",
	        "title": "Add OAuth",
	        "body": "please review",
	        "state": "OPEN",
	        "reviewDecision": "APPROVED",
	        "mergeable": "MERGEABLE",
	        "mergeStateStatus": "CLEAN",
	        "isDraft": false,
	        "url": "https://github.com/acme/app/pull/12",
	        "headRefOid": "abc123",
	        "headRefName": "oauth",
	        "baseRefName": "main",
	        "createdAt": "2026-08-20T10:00:00Z",
	        "updatedAt": "2026-08-21T11:45:00Z",
	        "author": {"login": "loopbot", "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4"},
	        "reactionGroups": [
	          {"content":"HOORAY","viewerHasReacted":false,"reactors":{"totalCount":3}}
	        ],
	        "reviewRequests": {"nodes":[
	          {"requestedReviewer":{"__typename":"User","login":"avery","avatarUrl":"https://avatars.githubusercontent.com/u/2?v=4"}},
	          {"requestedReviewer":{"__typename":"Bot","login":"copilot-pull-request-reviewer","avatarUrl":"https://avatars.githubusercontent.com/in/846209?v=4"}}
	        ]},
	        "assignees": {"nodes":[{"login":"sam","avatarUrl":"https://avatars.githubusercontent.com/u/3?v=4"}]},
	        "comments": {"nodes":[
	          {"id":"IC_1","databaseId":1001,"url":"https://github.com/acme/app/pull/12#issuecomment-1001","author":{"login":"avery"},"body":"looks good","createdAt":"2026-08-21T11:00:00Z",
	            "reactionGroups":[{"content":"THUMBS_UP","viewerHasReacted":true,"reactors":{"totalCount":1}}]}
	        ]},
	        "reviews": {"nodes":[
	          {"id":"PRR_1","databaseId":99,"url":"https://github.com/acme/app/pull/12#pullrequestreview-99","author":{"login":"sam"},"body":"ship it","state":"APPROVED","submittedAt":"2026-08-21T11:30:00Z"}
	        ]},
	        "commits": {"nodes":[
	          {"commit":{"oid":"abc123","url":"https://github.com/acme/app/commit/abc123","messageHeadline":"Add OAuth","messageBody":"","committedDate":"2026-08-20T10:01:00Z","signature":{"isValid":true},"statusCheckRollup":{"state":"SUCCESS"},"authors":{"nodes":[{"name":"Loop Bot","user":{"login":"loopbot"}}]}}}
	        ]},
	        "reviewThreads": {"nodes":[
	          {"id":"PRRT_1","isResolved":true,"diffSide":"RIGHT","startDiffSide":"RIGHT","comments":{"nodes":[
	            {"id":"PRRC_1","databaseId":42,"url":"https://github.com/acme/app/pull/12#discussion_r42","author":{"login":"avery"},"body":"nit","createdAt":"2026-08-21T11:10:00Z","path":"auth.go","line":12,"originalLine":12,"outdated":true,"diffHunk":"@@ -12 +12 @@","pullRequestReview":{"databaseId":99},
	              "reactionGroups":[{"content":"EYES","viewerHasReacted":false,"reactors":{"totalCount":1}}]}
	          ]}}
	        ]}
	      }
	    }
	  }
	}`
	snap, err := parseGitHubPRSnapshotGraphQL(gql, now)
	if err != nil {
		t.Fatalf("parseGitHubPRSnapshotGraphQL() error = %v, want nil", err)
	}
	timeline := `[{"event":"copilot_work_started","created_at":"2026-08-21T11:40:00Z","actor":{"login":"avery"},"performed_via_github_app":{"slug":"copilot-swe-agent"}}]`
	snap = finishGitHubPRSnapshot(snap, timeline, now)

	if snap.Title != "Add OAuth" || snap.State != "OPEN" || snap.AuthorLogin != "loopbot" || snap.NodeID != "PR_1" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() snapshot = %+v, want Add OAuth OPEN loopbot PR_1", snap)
	}
	if snap.ViewerLogin != "loopbot" || !snap.ViewerCanWrite {
		t.Errorf("parseGitHubPRSnapshotGraphQL() viewer = login %q write %v, want loopbot true", snap.ViewerLogin, snap.ViewerCanWrite)
	}
	if len(snap.AllowedMergeMethods) != 1 || snap.AllowedMergeMethods[0] != string(domain.MergeMethodSquash) {
		t.Errorf("parseGitHubPRSnapshotGraphQL() AllowedMergeMethods = %v, want [squash]", snap.AllowedMergeMethods)
	}
	if len(snap.ReviewRequests) != 2 || snap.ReviewRequests[0].Login != "avery" || snap.ReviewRequests[1].Login != "copilot-pull-request-reviewer" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() reviewRequests = %+v, want avery and copilot-pull-request-reviewer", snap.ReviewRequests)
	}
	if len(snap.Assignees) != 1 || snap.Assignees[0].Login != "sam" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() assignees = %+v, want sam", snap.Assignees)
	}
	meta, ok := snap.commitMeta["abc123"]
	if !ok || !meta.Verified || meta.CheckStatus != "pass" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() commitMeta[abc123] = %+v ok %v, want verified pass", meta, ok)
	}

	desc, ok := snapshotCommentByID(snap.Comments, "gh-description")
	if !ok {
		t.Fatal("parseGitHubPRSnapshotGraphQL() missing gh-description")
	}
	if !domain.ReactionsEqual(desc.Reactions, []domain.PRReaction{{Content: "hooray", Count: 3}}) {
		t.Errorf("parseGitHubPRSnapshotGraphQL() description reactions = %+v, want hooray×3", desc.Reactions)
	}
	comment, ok := snapshotCommentByID(snap.Comments, "gh-comment-IC_1")
	if !ok {
		t.Fatal("parseGitHubPRSnapshotGraphQL() missing gh-comment-IC_1")
	}
	if comment.BodyMD != "looks good" || !domain.ReactionsEqual(comment.Reactions, []domain.PRReaction{{Content: "+1", Count: 1, Mine: true}}) {
		t.Errorf("parseGitHubPRSnapshotGraphQL() comment = %+v, want looks good +1 mine", comment)
	}
	review, ok := snapshotCommentByID(snap.Comments, "gh-review-PRR_1")
	if !ok {
		t.Fatal("parseGitHubPRSnapshotGraphQL() missing gh-review-PRR_1")
	}
	if review.BodyMD != "ship it" || review.Verb != "approved" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() review = %+v, want ship it approved", review)
	}
	inline, ok := snapshotCommentByID(snap.Comments, "gh-inline-42")
	if !ok {
		t.Fatal("parseGitHubPRSnapshotGraphQL() missing gh-inline-42")
	}
	if inline.Path != "auth.go" || inline.Line != 12 || inline.Side != "RIGHT" || !inline.Outdated || inline.ThreadID != "PRRT_1" || !inline.Resolved {
		t.Errorf("parseGitHubPRSnapshotGraphQL() inline = %+v, want auth.go:12 RIGHT outdated resolved PRRT_1", inline)
	}
	if !domain.ReactionsEqual(inline.Reactions, []domain.PRReaction{{Content: "eyes", Count: 1}}) {
		t.Errorf("parseGitHubPRSnapshotGraphQL() inline reactions = %+v, want eyes×1", inline.Reactions)
	}
	if inline.ReviewID != "99" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() inline ReviewID = %q, want 99", inline.ReviewID)
	}

	var copilot domain.PRComment
	for _, c := range snap.Comments {
		if c.Kind == "action" && c.BodyMD == "started work on behalf of avery" {
			copilot = c
			break
		}
	}
	if copilot.ID == "" || copilot.Author != "Copilot" {
		t.Errorf("finishGitHubPRSnapshot() Copilot event = %+v, want Copilot started work on behalf of avery", copilot)
	}
}

func TestParseGitHubPRSnapshotGraphQLLeftSideThread(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	gql := `{
	  "data": {
	    "repository": {
	      "pullRequest": {
	        "id": "PR_1",
	        "title": "Fix",
	        "url": "https://github.com/acme/app/pull/12",
	        "reviewThreads": {"nodes":[
	          {"id":"PRRT_L","diffSide":"LEFT","startDiffSide":"LEFT","comments":{"nodes":[
	            {"databaseId":7,"url":"https://github.com/acme/app/pull/12#discussion_r7","author":{"login":"avery"},"body":"gone","createdAt":"2026-08-21T11:10:00Z","path":"old.go","line":9,"originalLine":9,"startLine":7,"originalStartLine":7,"diffHunk":"@@ -7,3 +7,3 @@"}
	          ]}}
	        ]}
	      }
	    }
	  }
	}`
	snap, err := parseGitHubPRSnapshotGraphQL(gql, now)
	if err != nil {
		t.Fatalf("parseGitHubPRSnapshotGraphQL() error = %v, want nil", err)
	}
	inline, ok := snapshotCommentByID(snap.Comments, "gh-inline-7")
	if !ok {
		t.Fatal("parseGitHubPRSnapshotGraphQL() missing gh-inline-7")
	}
	if inline.Side != "LEFT" || inline.StartSide != "LEFT" || inline.Path != "old.go" || inline.Line != 9 || inline.StartLine != 7 {
		t.Errorf("parseGitHubPRSnapshotGraphQL() left inline = %+v, want LEFT old.go:7-9", inline)
	}
}

func TestParseGitHubPRSnapshotGraphQLCheckPermissionError(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	gql := `{
	  "data": {
	    "viewer": {"login": "loopbot"},
	    "repository": {
	      "viewerPermission": "WRITE",
	      "pullRequest": {
	        "id": "PR_1",
	        "title": "Add OAuth",
	        "state": "OPEN",
	        "url": "https://github.com/acme/app/pull/12",
	        "author": {"login": "loopbot"},
	        "comments": {"nodes":[
	          {"id":"IC_1","url":"https://github.com/acme/app/pull/12#issuecomment-1001","author":{"login":"avery"},"body":"looks good","createdAt":"2026-08-21T11:00:00Z"}
	        ]},
	        "commits": {"nodes":[
	          {"commit":{"oid":"abc123","messageHeadline":"Add OAuth","committedDate":"2026-08-20T10:01:00Z","authors":{"nodes":[{"name":"Loop Bot","user":{"login":"loopbot"}}]}}}
	        ]}
	      }
	    }
	  },
	  "errors": [
	    {"message":"Resource not accessible by personal access token","path":["repository","pullRequest","commits","nodes",0,"commit","statusCheckRollup"]}
	  ]
	}`
	snap, err := parseGitHubPRSnapshotGraphQL(gql, now)
	if err != nil {
		t.Fatalf("parseGitHubPRSnapshotGraphQL() error = %v, want nil", err)
	}
	if snap.Title != "Add OAuth" || snap.State != "OPEN" {
		t.Errorf("parseGitHubPRSnapshotGraphQL() snapshot = %+v, want Add OAuth OPEN", snap)
	}
	if _, ok := snapshotCommentByID(snap.Comments, "gh-comment-IC_1"); !ok {
		t.Error("parseGitHubPRSnapshotGraphQL() missing gh-comment-IC_1")
	}
}

func TestParseGitHubPRSnapshotGraphQLTruncatedComments(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	gql := `{
	  "data": {
	    "repository": {
	      "pullRequest": {
	        "id": "PR_1",
	        "title": "Busy",
	        "url": "https://github.com/acme/app/pull/12",
	        "comments": {"pageInfo": {"hasPreviousPage": true}, "nodes":[
	          {"id":"IC_new","url":"https://github.com/acme/app/pull/12#issuecomment-2","author":{"login":"avery"},"body":"newest","createdAt":"2026-08-21T11:00:00Z"}
	        ]},
	        "reviews": {"pageInfo": {"hasPreviousPage": true}, "nodes":[]}
	      }
	    }
	  }
	}`
	snap, err := parseGitHubPRSnapshotGraphQL(gql, now)
	if err != nil {
		t.Fatalf("parseGitHubPRSnapshotGraphQL() error = %v, want nil", err)
	}
	if !snap.commentsTruncated || !snap.reviewsTruncated {
		t.Errorf("parseGitHubPRSnapshotGraphQL() truncated comments=%v reviews=%v, want true true", snap.commentsTruncated, snap.reviewsTruncated)
	}
}

func TestGitHubPRSnapshotGraphQLQueryOmitsRollup(t *testing.T) {
	t.Parallel()
	if !strings.Contains(githubPRSnapshotGraphQLQuery(true), "statusCheckRollup") {
		t.Error("githubPRSnapshotGraphQLQuery(true) missing statusCheckRollup, want field present")
	}
	got := githubPRSnapshotGraphQLQuery(false)
	if strings.Contains(got, "statusCheckRollup") {
		t.Errorf("githubPRSnapshotGraphQLQuery(false) = %q, want no statusCheckRollup", got)
	}
	if !strings.Contains(got, "reviewThreads") || !strings.Contains(got, "comments") {
		t.Errorf("githubPRSnapshotGraphQLQuery(false) = %q, want core PR fields", got)
	}
	if !strings.Contains(got, "pageInfo { hasPreviousPage }") {
		t.Error("githubPRSnapshotGraphQLQuery(false) missing comments/reviews pageInfo, want hasPreviousPage")
	}
}

func TestParseGitHubPRSnapshotGraphQLEmpty(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	if _, err := parseGitHubPRSnapshotGraphQL("", now); err == nil {
		t.Error("parseGitHubPRSnapshotGraphQL(\"\") error = nil, want error")
	}
	if _, err := parseGitHubPRSnapshotGraphQL(`{"data":{"repository":{}},"errors":[{"message":"Could not resolve to a PullRequest"}]}`, now); err == nil {
		t.Error("parseGitHubPRSnapshotGraphQL(missing PR) error = nil, want error")
	}
}

func TestGitHubPRTimelineAPIArgsPaginates(t *testing.T) {
	t.Parallel()
	got := githubPRTimelineAPIArgs("acme", "app", 12)
	want := []string{"api", "--paginate", "--slurp", "repos/acme/app/issues/12/timeline?per_page=100", "-H", "Accept: application/vnd.github+json"}
	if strings.Join(got, " ") != strings.Join(want, " ") {
		t.Errorf("githubPRTimelineAPIArgs(acme, app, 12) = %q, want %q", got, want)
	}
}

func TestFlattenJSONArrayPages(t *testing.T) {
	t.Parallel()
	flat := `[{"event":"committed","sha":"aaa"}]`
	if got := flattenJSONArrayPages(flat); got != flat {
		t.Errorf("flattenJSONArrayPages(flat) = %s, want %q", got, flat)
	}
	slurped := `[[{"event":"committed","sha":"old"}],[{"event":"committed","sha":"new"}]]`
	got := flattenJSONArrayPages(slurped)
	var rows []struct {
		SHA string `json:"sha"`
	}
	if err := json.Unmarshal([]byte(got), &rows); err != nil {
		t.Fatalf("flattenJSONArrayPages(slurped) json = %s error = %v, want array", got, err)
	}
	if len(rows) != 2 || rows[0].SHA != "old" || rows[1].SHA != "new" {
		t.Errorf("flattenJSONArrayPages(slurped) = %s, want old then new", got)
	}
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	acts := parseGitHubTimeline(got, now, nil, nil, "", "", "")
	if len(acts) != 2 || acts[0].CommitSHA != "old" || acts[1].CommitSHA != "new" {
		t.Errorf("parseGitHubTimeline(flattenJSONArrayPages(slurped)) = %+v, want committed old then new", acts)
	}
}

func TestGitHubPermissionCanWrite(t *testing.T) {
	t.Parallel()
	if !githubPermissionCanWrite("WRITE") || !githubPermissionCanWrite("admin") || !githubPermissionCanWrite("MAINTAIN") {
		t.Error("githubPermissionCanWrite(WRITE/ADMIN/MAINTAIN) = false, want true")
	}
	if githubPermissionCanWrite("READ") || githubPermissionCanWrite("TRIAGE") || githubPermissionCanWrite("") {
		t.Error("githubPermissionCanWrite(READ/TRIAGE/empty) = true, want false")
	}
}

func snapshotCommentByID(cs []domain.PRComment, id string) (domain.PRComment, bool) {
	for _, c := range cs {
		if c.ID == id {
			return c, true
		}
	}
	return domain.PRComment{}, false
}
