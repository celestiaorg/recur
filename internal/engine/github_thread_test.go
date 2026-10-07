package engine

import (
	"context"
	"encoding/base64"
	"encoding/binary"
	"fmt"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/domain"
)

func encodeGitHubNodeID(prefix string, typeID uint32, dbID uint64) string {
	raw := make([]byte, 12)
	binary.BigEndian.PutUint32(raw[0:4], typeID)
	binary.BigEndian.PutUint64(raw[4:12], dbID)
	return prefix + "_" + strings.TrimRight(base64.StdEncoding.EncodeToString(raw), "=")
}

func TestGitHubNodeDatabaseID(t *testing.T) {
	t.Parallel()
	if n, ok := githubNodeDatabaseID("99"); !ok || n != 99 {
		t.Errorf("githubNodeDatabaseID(99) = %d %v, want 99 true", n, ok)
	}
	id := encodeGitHubNodeID("PRR", 8, 3864739160)
	if n, ok := githubNodeDatabaseID(id); !ok || n != 3864739160 {
		t.Errorf("githubNodeDatabaseID(%s) = %d %v, want 3864739160 true", id, n, ok)
	}
	legacy := base64.StdEncoding.EncodeToString([]byte("012:PullRequestReview42"))
	if n, ok := githubNodeDatabaseID(legacy); !ok || n != 42 {
		t.Errorf("githubNodeDatabaseID(legacy) = %d %v, want 42 true", n, ok)
	}
	if _, ok := githubNodeDatabaseID("PRR_c"); ok {
		t.Error("githubNodeDatabaseID(PRR_c) ok = true, want false")
	}
	got := githubPermalink("https://github.com/acme/app/pull/1/", "pullrequestreview", id, "")
	if got != "https://github.com/acme/app/pull/1#pullrequestreview-3864739160" {
		t.Errorf("githubPermalink() = %q, want review fragment", got)
	}
}

func TestParseGitHubPRView(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	prNode := encodeGitHubNodeID("PR", 5, 5215972950)
	raw := fmt.Sprintf(`{
		"id": %q,
		"title": "Add OAuth",
		"state": "OPEN",
		"reviewDecision": "APPROVED",
		"headRefOid": "abc",
		"url": "https://github.com/acme/app/pull/12",
		"createdAt": "2026-08-20T10:00:00Z",
		"updatedAt": "2026-08-21T11:45:00Z",
		"author": {"login": "loopbot"},
		"reviewRequests": [{"login": "avery"}],
		"comments": [
			{"id": "IC_1", "url": "https://github.com/acme/app/pull/12#issuecomment-1", "author": {"login": "avery"}, "body": "looks good", "createdAt": "2026-08-21T11:00:00Z"}
		],
		"reviews": [
			{"id": "PRR_1", "url": "https://github.com/acme/app/pull/12#pullrequestreview-1", "author": {"login": "sam"}, "body": "ship it", "state": "APPROVED", "submittedAt": "2026-08-21T11:30:00Z"},
			{"id": "PRR_2", "author": {"login": "bot"}, "body": "", "state": "PENDING", "submittedAt": "2026-08-21T11:31:00Z"}
		]
	}`, prNode)
	snap, err := parseGitHubPRView(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRView() error = %v, want nil", err)
	}
	if snap.Title != "Add OAuth" || snap.State != "OPEN" || snap.ReviewDecision != "APPROVED" || snap.AuthorLogin != "loopbot" {
		t.Errorf("parseGitHubPRView() snapshot = %+v, want title Add OAuth OPEN APPROVED author loopbot", snap)
	}
	if snap.CreatedAt != "2026-08-20T10:00:00Z" || snap.UpdatedAt != "2026-08-21T11:45:00Z" {
		t.Errorf("parseGitHubPRView() times = created %q updated %q, want createdAt and updatedAt from gh", snap.CreatedAt, snap.UpdatedAt)
	}
	if len(snap.ReviewRequests) != 1 || snap.ReviewRequests[0].Login != "avery" {
		t.Errorf("parseGitHubPRView() reviewRequests = %+v, want avery", snap.ReviewRequests)
	}
	if _, ok := ownerRepoFromPRURL(snap.URL); !ok {
		t.Errorf("ownerRepoFromPRURL(%q) ok = false, want true", snap.URL)
	}
	if len(snap.Comments) != 3 {
		t.Fatalf("parseGitHubPRView() comments = %d, want 3 (description + comment + review with body; pending skipped)", len(snap.Comments))
	}
	if snap.Comments[0].Kind != "description" || snap.Comments[0].ID != "gh-description" {
		t.Errorf("parseGitHubPRView() comment[0] = %+v, want description gh-description", snap.Comments[0])
	}
	if snap.Comments[0].URL != "https://github.com/acme/app/pull/12#issue-5215972950" {
		t.Errorf("parseGitHubPRView() description URL = %q, want #issue- permalink", snap.Comments[0].URL)
	}
	if snap.Comments[1].Author != "avery" || snap.Comments[1].ID != "gh-comment-IC_1" || snap.Comments[1].AuthorLogin != "avery" {
		t.Errorf("parseGitHubPRView() comment[1] = %+v, want avery gh-comment-IC_1", snap.Comments[1])
	}
	if snap.Comments[1].URL != "https://github.com/acme/app/pull/12#issuecomment-1" {
		t.Errorf("parseGitHubPRView() comment[1].URL = %q, want issuecomment permalink", snap.Comments[1].URL)
	}
	if snap.Comments[1].BodyMD != "looks good" {
		t.Errorf("parseGitHubPRView() comment[1].BodyMD = %q, want looks good", snap.Comments[1].BodyMD)
	}
	if snap.Comments[1].Verb != "commented" {
		t.Errorf("parseGitHubPRView() comment[1].Verb = %q, want commented", snap.Comments[1].Verb)
	}
	if snap.Comments[2].Author != "sam" || snap.Comments[2].ID != "gh-review-PRR_1" || snap.Comments[2].BodyMD != "ship it" {
		t.Errorf("parseGitHubPRView() comment[2] = %+v, want sam gh-review-PRR_1 ship it", snap.Comments[2])
	}
	if snap.Comments[2].Verb != "approved" || snap.Comments[2].Kind != "comment" {
		t.Errorf("parseGitHubPRView() comment[2] kind/verb = %q %q, want comment approved", snap.Comments[2].Kind, snap.Comments[2].Verb)
	}
	if snap.Comments[2].URL != "https://github.com/acme/app/pull/12#pullrequestreview-1" {
		t.Errorf("parseGitHubPRView() comment[2].URL = %q, want review permalink", snap.Comments[2].URL)
	}
}

func TestParseGitHubPRViewBodylessReview(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	raw := `{
		"title": "PR",
		"state": "OPEN",
		"url": "https://github.com/acme/app/pull/12",
		"author": {"login": "loopbot"},
		"reviews": [
			{"id": "PRR_9", "author": {"login": "sam"}, "body": "", "state": "CHANGES_REQUESTED", "submittedAt": "2026-08-21T11:30:00Z"}
		]
	}`
	snap, err := parseGitHubPRView(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRView() error = %v, want nil", err)
	}
	if len(snap.Comments) != 2 {
		t.Fatalf("parseGitHubPRView() comments = %d, want 2 (description + action)", len(snap.Comments))
	}
	got := snap.Comments[1]
	if got.Kind != "action" || got.Author != "sam" || got.BodyMD != "requested changes" {
		t.Errorf("parseGitHubPRView() = %+v, want sam action requested changes", got)
	}
}

func TestParseGitHubPRViewBotMarkdown(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	nodeID := encodeGitHubNodeID("PRR", 8, 3864739160)
	raw := fmt.Sprintf(`{
		"title": "PR",
		"state": "OPEN",
		"reviewDecision": "",
		"headRefOid": "abc",
		"url": "https://github.com/acme/app/pull/1",
		"comments": [],
		"reviews": [
			{"id": %q, "author": {"login": "copilot-pull-request-reviewer", "type": "Bot", "avatarUrl": "https://avatars.githubusercontent.com/in/946600?v=4"}, "body": "## Pull request overview\n\n**Changes:** table", "state": "COMMENTED", "submittedAt": "2026-08-21T11:30:00Z"}
		]
	}`, nodeID)
	snap, err := parseGitHubPRView(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRView() error = %v, want nil", err)
	}
	wantID := "gh-review-" + nodeID
	var review domain.PRComment
	for _, c := range snap.Comments {
		if c.ID == wantID {
			review = c
			break
		}
	}
	if review.ID == "" {
		t.Fatalf("parseGitHubPRView() comments = %+v, want %s", snap.Comments, wantID)
	}
	if review.URL != "https://github.com/acme/app/pull/1#pullrequestreview-3864739160" {
		t.Errorf("parseGitHubPRView() Copilot review URL = %q, want pullrequestreview permalink", review.URL)
	}
	if review.Author != "Copilot" {
		t.Errorf("parseGitHubPRView() review author = %q, want Copilot", review.Author)
	}
	if review.BodyMD != "## Pull request overview\n\n**Changes:** table" {
		t.Errorf("parseGitHubPRView() review BodyMD = %q, want source markdown", review.BodyMD)
	}
	if strings.Contains(review.BodyHTML, "##") {
		t.Errorf("parseGitHubPRView() review BodyHTML = %q, must not pre-escape markdown", review.BodyHTML)
	}
	if review.AvatarURL != "https://avatars.githubusercontent.com/in/946600?v=4" {
		t.Errorf("parseGitHubPRView() review AvatarURL = %q, want GitHub App icon /in/946600", review.AvatarURL)
	}
}

func TestParseGitHubInlineComments(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	raw := `[{"id": 99, "html_url": "https://github.com/acme/app/pull/12#discussion_r99", "body": "nits", "path": "auth.go", "line": 12, "side": "RIGHT", "diff_hunk": "@@ -12,1 +12,1 @@\n-old\n+new", "created_at": "2026-08-21T11:55:00Z", "pull_request_review_id": 42, "user": {"login": "dependabot[bot]", "type": "Bot", "avatar_url": "https://avatars.githubusercontent.com/in/29110?v=4"}}]`
	got := parseGitHubInlineComments(raw, now, "https://github.com/acme/app/pull/12")
	if len(got) != 1 {
		t.Fatalf("parseGitHubInlineComments() len = %d, want 1", len(got))
	}
	if got[0].ID != "gh-inline-99" || got[0].Author != "dependabot" {
		t.Errorf("parseGitHubInlineComments() = %+v, want dependabot gh-inline-99", got[0])
	}
	if got[0].BodyMD != "nits" {
		t.Errorf("parseGitHubInlineComments() BodyMD = %q, want nits", got[0].BodyMD)
	}
	if !strings.Contains(got[0].BodyHTML, "auth.go:12") {
		t.Errorf("parseGitHubInlineComments() BodyHTML = %q, want auth.go:12", got[0].BodyHTML)
	}
	if got[0].AvatarURL != "https://avatars.githubusercontent.com/in/29110?v=4" {
		t.Errorf("parseGitHubInlineComments() AvatarURL = %q, want GitHub App icon /in/29110", got[0].AvatarURL)
	}
	if got[0].URL != "https://github.com/acme/app/pull/12#discussion_r99" {
		t.Errorf("parseGitHubInlineComments() URL = %q, want discussion permalink", got[0].URL)
	}
	if got[0].ReviewID != "42" {
		t.Errorf("parseGitHubInlineComments() ReviewID = %q, want 42", got[0].ReviewID)
	}
	if got[0].Path != "auth.go" || got[0].Line != 12 || !strings.Contains(got[0].DiffHunk, "+new") {
		t.Errorf("parseGitHubInlineComments() path/hunk = %q %d %q", got[0].Path, got[0].Line, got[0].DiffHunk)
	}
}

func TestParseGitHubInlineCommentsMultilineAndOutdated(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	raw := `[
		{"id": 1, "body": "span", "path": "a.go", "line": 20, "start_line": 16, "side": "RIGHT", "start_side": "RIGHT", "created_at": "2026-08-21T11:55:00Z", "user": {"login": "avery"}},
		{"id": 2, "body": "stale", "path": "b.go", "line": null, "original_line": 9, "start_line": null, "original_start_line": 7, "side": "LEFT", "created_at": "2026-08-21T11:56:00Z", "user": {"login": "sam"}}
	]`
	got := parseGitHubInlineComments(raw, now, "https://github.com/acme/app/pull/12")
	if len(got) != 2 {
		t.Fatalf("len = %d, want 2", len(got))
	}
	if got[0].Line != 20 || got[0].StartLine != 16 || got[0].StartSide != "RIGHT" {
		t.Errorf("multiline = line %d start %d side %q", got[0].Line, got[0].StartLine, got[0].StartSide)
	}
	if !strings.Contains(got[0].BodyHTML, "a.go:16-20") {
		t.Errorf("BodyHTML = %q, want a.go:16-20", got[0].BodyHTML)
	}
	if got[1].Line != 9 || got[1].StartLine != 7 || got[1].Side != "LEFT" || !got[1].Outdated {
		t.Errorf("outdated = line %d start %d side %q outdated %v", got[1].Line, got[1].StartLine, got[1].Side, got[1].Outdated)
	}
	if got[0].Outdated {
		t.Error("multiline comment Outdated = true, want false")
	}
	if !strings.Contains(got[1].BodyHTML, "b.go:7-9") || !strings.Contains(got[1].BodyHTML, "outdated") {
		t.Errorf("outdated BodyHTML = %q, want b.go:7-9 (outdated)", got[1].BodyHTML)
	}
}

func TestParsePostedInlineComment(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	raw := `{"id": 999, "html_url": "https://github.com/acme/app/pull/12#discussion_r999", "body": "thread reply", "path": "auth.go", "line": 12, "in_reply_to_id": 1, "created_at": "2026-08-21T11:59:00Z", "user": {"login": "avery"}}`
	got, ok := parsePostedInlineComment(raw, now, "https://github.com/acme/app/pull/12")
	if !ok {
		t.Fatal("parsePostedInlineComment() ok = false, want true")
	}
	if got.ID != "gh-inline-999" || got.InReplyToID != "gh-inline-1" || got.BodyMD != "thread reply" {
		t.Errorf("parsePostedInlineComment() = %+v, want gh-inline-999 reply to gh-inline-1", got)
	}
	if _, ok := parsePostedInlineComment("[]", now, ""); ok {
		t.Error("parsePostedInlineComment([]) ok = true, want false")
	}
}

func TestAttachReviewComments(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	nodeID := encodeGitHubNodeID("PRR", 8, 99)
	raw := fmt.Sprintf(`{
		"id": "PR_1",
		"title": "Add OAuth",
		"state": "OPEN",
		"url": "https://github.com/acme/app/pull/12",
		"author": {"login": "loopbot"},
		"createdAt": "2026-08-21T10:00:00Z",
		"reviews": [
			{"id": %q, "url": "https://github.com/acme/app/pull/12#pullrequestreview-99", "author": {"login": "sam"}, "body": "testing again", "state": "COMMENTED", "submittedAt": "2026-08-21T11:30:00Z"},
			{"id": "PRR_empty", "url": "https://github.com/acme/app/pull/12#pullrequestreview-100", "author": {"login": "sam"}, "body": "", "state": "COMMENTED", "submittedAt": "2026-08-21T11:40:00Z"}
		]
	}`, nodeID)
	snap, err := parseGitHubPRView(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRView() error = %v, want nil", err)
	}
	inlines := parseGitHubInlineComments(`[
		{"id": 1, "body": "hmm", "path": "apps/bridge/.env.example", "line": 17, "created_at": "2026-08-21T11:30:01Z", "pull_request_review_id": 99, "user": {"login": "sam"}},
		{"id": 2, "body": "later", "path": "a.go", "line": 1, "created_at": "2026-08-21T11:40:01Z", "pull_request_review_id": 100, "user": {"login": "sam"}},
		{"id": 3, "body": "orphan", "path": "b.go", "line": 2, "created_at": "2026-08-21T11:50:00Z", "user": {"login": "pat"}},
		{"id": 4, "body": "reply", "path": "apps/bridge/.env.example", "line": 17, "created_at": "2026-08-21T11:35:00Z", "pull_request_review_id": 101, "in_reply_to_id": 1, "user": {"login": "pat"}}
	]`, now, "https://github.com/acme/app/pull/12")
	comments := append(snap.Comments, inlines...)
	attachReviewComments(comments)

	byID := map[string]domain.PRComment{}
	for _, c := range comments {
		byID[c.ID] = c
	}
	parent := byID["gh-review-"+nodeID]
	if parent.BodyMD != "testing again" {
		t.Fatalf("review body = %q, want testing again", parent.BodyMD)
	}
	if byID["gh-inline-1"].ParentID != parent.ID {
		t.Errorf("gh-inline-1.ParentID = %q, want %s", byID["gh-inline-1"].ParentID, parent.ID)
	}

	empty, ok := byID["gh-review-PRR_empty"]
	if !ok {
		t.Fatalf("empty-body review not promoted; comments = %+v", comments)
	}
	if empty.Kind != "comment" || empty.Verb != "commented" {
		t.Errorf("promoted review kind/verb = %q %q, want comment commented", empty.Kind, empty.Verb)
	}
	if empty.BodyMD != "" {
		t.Errorf("promoted review BodyMD = %q, want empty", empty.BodyMD)
	}
	if byID["gh-inline-2"].ParentID != empty.ID {
		t.Errorf("gh-inline-2.ParentID = %q, want %s", byID["gh-inline-2"].ParentID, empty.ID)
	}
	if byID["gh-inline-3"].ParentID != "" {
		t.Errorf("orphan ParentID = %q, want empty", byID["gh-inline-3"].ParentID)
	}
	if _, stillAction := byID["gh-review-action-PRR_empty"]; stillAction {
		t.Error("empty-body review stayed as gh-review-action, want promoted gh-review card")
	}
	if byID["gh-inline-4"].InReplyToID != "gh-inline-1" {
		t.Errorf("reply InReplyToID = %q, want gh-inline-1", byID["gh-inline-4"].InReplyToID)
	}
	if byID["gh-inline-4"].ParentID != parent.ID {
		t.Errorf("reply ParentID = %q, want %s", byID["gh-inline-4"].ParentID, parent.ID)
	}
}

func TestParseAndApplyReviewThreads(t *testing.T) {
	t.Parallel()
	raw := `{
		"data": {
			"repository": {
				"pullRequest": {
					"reviewThreads": {
						"nodes": [
							{"id": "PRRT_1", "isResolved": true, "comments": {"nodes": [{"databaseId": 1}, {"databaseId": 4}]}},
							{"id": "PRRT_2", "isResolved": false, "comments": {"nodes": [{"databaseId": 2}]}}
						]
					}
				}
			}
		}
	}`
	threads := parseGitHubReviewThreads(raw)
	if len(threads) != 2 || threads[0].ID != "PRRT_1" || !threads[0].Resolved || len(threads[0].CommentIDs) != 2 {
		t.Fatalf("parseGitHubReviewThreads() = %+v, want PRRT_1 resolved with 2 comments", threads)
	}
	cs := []domain.PRComment{
		{ID: "gh-inline-1"},
		{ID: "gh-inline-4"},
		{ID: "gh-inline-2"},
		{ID: "gh-comment-9"},
	}
	applyReviewThreads(cs, threads)
	if cs[0].ThreadID != "PRRT_1" || !cs[0].Resolved {
		t.Errorf("applyReviewThreads inline-1 = %+v, want PRRT_1 resolved", cs[0])
	}
	if cs[1].ThreadID != "PRRT_1" || !cs[1].Resolved {
		t.Errorf("applyReviewThreads inline-4 = %+v, want PRRT_1 resolved", cs[1])
	}
	if cs[2].ThreadID != "PRRT_2" || cs[2].Resolved {
		t.Errorf("applyReviewThreads inline-2 = %+v, want PRRT_2 open", cs[2])
	}
	if cs[3].ThreadID != "" {
		t.Errorf("applyReviewThreads issue comment ThreadID = %q, want empty", cs[3].ThreadID)
	}
}

func TestParseGitHubTimeline(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	prURL := "https://github.com/acme/app/pull/12"
	raw := `[
		{"event": "committed", "sha": "3f1d02babcdef", "message": "Enable the tab\n\nbody", "author": {"name": "josh", "date": "2026-08-21T10:00:00Z"}},
		{"id": 88, "event": "review_requested", "created_at": "2026-08-21T10:05:00Z", "review_requester": {"login": "josh", "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"}, "requested_reviewer": {"login": "copilot-pull-request-reviewer", "type": "Bot"}},
		{"event": "commented", "created_at": "2026-08-21T10:06:00Z", "actor": {"login": "josh"}, "body": "skip me"},
		{"event": "reviewed", "submitted_at": "2026-08-21T10:07:00Z", "user": {"login": "copilot-pull-request-reviewer"}, "body": "dup"}
	]`
	got := parseGitHubTimeline(raw, now, nil, nil, prURL, "", "")
	if len(got) != 2 {
		t.Fatalf("parseGitHubTimeline() len = %d, want 2 (commented/reviewed skipped)", len(got))
	}
	if got[0].Kind != "action" || !strings.Contains(got[0].BodyMD, "Enable the tab") || got[0].CommitSHA != "3f1d02babcdef" {
		t.Errorf("parseGitHubTimeline() [0] = %+v, want committed action with title and sha", got[0])
	}
	if got[0].AvatarURL != "" {
		t.Errorf("parseGitHubTimeline() [0].AvatarURL = %q, want empty (git author is not a GitHub login)", got[0].AvatarURL)
	}
	if got[1].Author != "josh" || got[1].BodyMD != "requested a review from Copilot" {
		t.Errorf("parseGitHubTimeline() [1] = %+v, want josh requested a review from Copilot", got[1])
	}
	if got[1].AvatarURL != "https://avatars.githubusercontent.com/u/1?v=4" {
		t.Errorf("parseGitHubTimeline() [1].AvatarURL = %q, want review_requester avatar_url", got[1].AvatarURL)
	}
	if got[1].URL != "https://github.com/acme/app/pull/12#event-88" {
		t.Errorf("parseGitHubTimeline() [1].URL = %q, want #event-88 permalink", got[1].URL)
	}

	linked := parseGitHubTimeline(raw, now, map[string]ghViewActor{
		"3f1d02babcdef999": {Login: "mycodecrafting"},
	}, map[string]ghCommitMeta{
		"3f1d02babcdef999": {Verified: true, CheckStatus: "pass", URL: "https://github.com/acme/app/commit/3f1d02babcdef999"},
	}, prURL, "", "")
	if len(linked) != 2 {
		t.Fatalf("parseGitHubTimeline(linked) len = %d, want 2", len(linked))
	}
	if linked[0].Author != "mycodecrafting" {
		t.Errorf("parseGitHubTimeline(linked) [0].Author = %q, want mycodecrafting", linked[0].Author)
	}
	if linked[0].AvatarURL != "https://github.com/mycodecrafting.png?size=56" {
		t.Errorf("parseGitHubTimeline(linked) [0].AvatarURL = %q, want mycodecrafting png", linked[0].AvatarURL)
	}
	if !linked[0].Verified || linked[0].CheckStatus != "pass" || linked[0].CommitSHA != "3f1d02babcdef" {
		t.Errorf("parseGitHubTimeline(linked) [0] verified/checks/sha = %+v, want verified pass 3f1d02babcdef", linked[0])
	}
}

func TestParseGitHubTimelineCopilotWork(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 18, 0, 0, 0, time.UTC)
	raw := `[
		{"event":"copilot_work_started","created_at":"2026-08-21T17:12:10Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-pull-request-reviewer"}},
		{"event":"copilot_work_started","created_at":"2026-08-21T17:20:00Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-swe-agent"}},
		{"event":"copilot_work_finished","created_at":"2026-08-21T17:25:00Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-swe-agent"}},
		{"event":"copilot_work_finished_failure","created_at":"2026-08-21T17:26:00Z","actor":{"login":"mycodecrafting"}}
	]`
	got := parseGitHubTimeline(raw, now, nil, nil, "https://github.com/acme/app/pull/12", "", "")
	if len(got) != 4 {
		t.Fatalf("parseGitHubTimeline() len = %d, want 4 copilot work events", len(got))
	}
	want := []string{
		"started reviewing on behalf of mycodecrafting",
		"started work on behalf of mycodecrafting",
		"finished work on behalf of mycodecrafting",
		"stopped work on behalf of mycodecrafting due to an error",
	}
	for i, body := range want {
		if got[i].Author != "Copilot" || got[i].Kind != "action" || got[i].BodyMD != body {
			t.Errorf("parseGitHubTimeline() [%d] = %+v, want Copilot action %q", i, got[i], body)
		}
		if got[i].AvatarURL != "" {
			t.Errorf("parseGitHubTimeline() [%d].AvatarURL = %q, want empty (branch glyph, not avatar)", i, got[i].AvatarURL)
		}
	}
}

func TestParseGitHubTimelineMergedEvents(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 23, 12, 0, 0, 0, time.UTC)
	raw := `[
		{"event":"merged","commit_id":"a3f73e2abcdef","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}},
		{"event":"closed","commit_id":"a3f73e2abcdef","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}},
		{"event":"head_ref_deleted","created_at":"2026-08-23T04:00:05Z","actor":{"login":"mycodecrafting"}},
		{"event":"labeled","created_at":"2026-08-23T03:50:00Z","actor":{"login":"mycodecrafting"},"label":{"name":"ready"}},
		{"event":"renamed","created_at":"2026-08-23T03:40:00Z","actor":{"login":"mycodecrafting"},"rename":{"from":"Old title","to":"New title"}}
	]`
	got := parseGitHubTimeline(raw, now, nil, nil, "https://github.com/acme/app/pull/12", "mycodecrafting/screening-cache", "mycodecrafting/reskin")
	if len(got) != 4 {
		t.Fatalf("parseGitHubTimeline() len = %d, want 4 (closed-on-merge skipped)", len(got))
	}
	if got[0].BodyMD != "merged commit a3f73e2 into mycodecrafting/reskin" || got[0].AvatarURL != "" || got[0].CommitSHA != "a3f73e2abcdef" {
		t.Errorf("merge event = %+v, want merged commit into base with glyph (no avatar) and full sha", got[0])
	}
	if got[1].BodyMD != "deleted the mycodecrafting/screening-cache branch" || got[1].AvatarURL != "" {
		t.Errorf("head_ref_deleted = %+v, want deleted branch glyph event", got[1])
	}
	if got[2].BodyMD != "added the ready label" {
		t.Errorf("labeled = %q, want added the ready label", got[2].BodyMD)
	}
	if got[3].BodyMD != "changed the title from Old title to New title" {
		t.Errorf("renamed = %q, want title change", got[3].BodyMD)
	}

	closedOnly := parseGitHubTimeline(`[{"event":"closed","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}}]`, now, nil, nil, "", "", "")
	if len(closedOnly) != 1 || closedOnly[0].BodyMD != "closed this" {
		t.Errorf("closed-without-merge = %+v, want closed this", closedOnly)
	}
}

func TestParseGitHubCommitRollup(t *testing.T) {
	t.Parallel()
	raw := `{
		"data": {
			"repository": {
				"pullRequest": {
					"author": {"login": "loopbot", "avatarUrl": "https://avatars.githubusercontent.com/u/2?v=4"},
					"reviews": {"nodes": [{"author": {"login": "copilot-pull-request-reviewer", "avatarUrl": "https://avatars.githubusercontent.com/in/946600?v=4"}}]},
					"commits": {
						"nodes": [
							{"commit":{"oid":"aaa","url":"https://github.com/acme/app/commit/aaa","signature":{"isValid":true},"statusCheckRollup":{"state":"SUCCESS"}}},
							{"commit":{"oid":"bbb","url":"https://github.com/acme/app/commit/bbb","signature":null,"statusCheckRollup":{"state":"FAILURE"}}}
						]
					}
				}
			}
		}
	}`
	got, avatars := parseGitHubCommitRollup(raw)
	if !got["aaa"].Verified || got["aaa"].CheckStatus != "pass" || got["aaa"].URL == "" {
		t.Errorf("parseGitHubCommitRollup()[aaa] = %+v, want verified pass with url", got["aaa"])
	}
	if got["bbb"].Verified || got["bbb"].CheckStatus != "fail" {
		t.Errorf("parseGitHubCommitRollup()[bbb] = %+v, want unverified fail", got["bbb"])
	}
	if avatars["copilot-pull-request-reviewer"] != "https://avatars.githubusercontent.com/in/946600?v=4" {
		t.Errorf("parseGitHubCommitRollup() avatars[copilot-pull-request-reviewer] = %q, want GitHub App icon /in/946600", avatars["copilot-pull-request-reviewer"])
	}
	if avatars["copilot"] != "https://avatars.githubusercontent.com/in/946600?v=4" {
		t.Errorf("parseGitHubCommitRollup() avatars[copilot] = %q, want GitHub App icon keyed by display name", avatars["copilot"])
	}
}

func TestCommitAuthorsFromView(t *testing.T) {
	t.Parallel()
	got := commitAuthorsFromView([]ghViewCommit{
		{OID: "3f1d02babcdef", Authors: []ghViewCommitAuthor{{Name: "Josh"}, {Login: "mycodecrafting", Name: "Josh"}}},
		{OID: "deadbeef", Authors: []ghViewCommitAuthor{{Name: "no login"}}},
	})
	if got["3f1d02babcdef"].Login != "mycodecrafting" {
		t.Errorf("commitAuthorsFromView()[3f1d02babcdef] = %+v, want mycodecrafting", got["3f1d02babcdef"])
	}
	if _, ok := got["deadbeef"]; ok {
		t.Errorf("commitAuthorsFromView()[deadbeef] = %+v, want omitted (no GitHub login)", got["deadbeef"])
	}
}

func TestEnrichGitHubCommits(t *testing.T) {
	t.Parallel()
	got := enrichGitHubCommits([]domain.Commit{
		{Hash: "3f1d02b", SHA: "", Author: "Josh", AuthorRole: "Author"},
		{Hash: "deadbee", Author: "local"},
	}, map[string]ghViewActor{
		"3f1d02babcdef999": {Login: "mycodecrafting"},
	}, map[string]ghCommitMeta{
		"3f1d02babcdef999": {Verified: true, CheckStatus: "pass"},
	})
	if got[0].Author != "mycodecrafting" || got[0].AvatarURL == "" || !got[0].Verified || got[0].CheckStatus != "pass" {
		t.Errorf("enrichGitHubCommits()[0] = %+v, want mycodecrafting verified pass with avatar", got[0])
	}
	if got[1].Author != "local" || got[1].Verified || got[1].CheckStatus != "" {
		t.Errorf("enrichGitHubCommits()[1] = %+v, want unchanged local commit", got[1])
	}
}

func TestGHAvatarURL(t *testing.T) {
	t.Parallel()
	if got := ghAvatarURL("mycodecrafting", ""); got != "https://github.com/mycodecrafting.png?size=56" {
		t.Errorf("ghAvatarURL(mycodecrafting, \"\") = %q, want GitHub png", got)
	}
	if got := ghAvatarURL("dependabot[bot]", ""); got != "https://github.com/dependabot.png?size=56" {
		t.Errorf("ghAvatarURL(dependabot[bot], \"\") = %q, want dependabot png fallback", got)
	}
	if got := ghAvatarURL("dependabot[bot]", "https://avatars.githubusercontent.com/in/29110?v=4"); got != "https://avatars.githubusercontent.com/in/29110?v=4" {
		t.Errorf("ghAvatarURL(dependabot[bot], app icon) = %q, want GitHub App icon /in/29110", got)
	}
	if got := ghAvatarURL("A Name", ""); got != "" {
		t.Errorf("ghAvatarURL(A Name, \"\") = %q, want empty", got)
	}
}

func TestApplyGHAvatarsUsesAppIcon(t *testing.T) {
	t.Parallel()
	comments := []domain.PRComment{
		{ID: "gh-review-1", Author: "Copilot", AuthorLogin: "copilot-pull-request-reviewer", AvatarURL: "https://github.com/copilot-pull-request-reviewer.png?size=56"},
		{ID: "gh-event-work", Author: "Copilot", AuthorLogin: "copilot-pull-request-reviewer", AvatarURL: ""},
		{ID: "gh-comment-1", Author: "avery", AuthorLogin: "avery", AvatarURL: "https://github.com/avery.png?size=56"},
	}
	applyGHAvatars(comments, map[string]string{
		"copilot-pull-request-reviewer": "https://avatars.githubusercontent.com/in/946600?v=4",
		"copilot":                       "https://avatars.githubusercontent.com/in/946600?v=4",
		"avery":                         "https://avatars.githubusercontent.com/u/9?v=4",
	})
	if comments[0].AvatarURL != "https://avatars.githubusercontent.com/in/946600?v=4" {
		t.Errorf("applyGHAvatars() review AvatarURL = %q, want GitHub App icon /in/946600", comments[0].AvatarURL)
	}
	if comments[1].AvatarURL != "" {
		t.Errorf("applyGHAvatars() work event AvatarURL = %q, want empty (branch glyph)", comments[1].AvatarURL)
	}
	if comments[2].AvatarURL != "https://avatars.githubusercontent.com/u/9?v=4" {
		t.Errorf("applyGHAvatars() human AvatarURL = %q, want API avatar", comments[2].AvatarURL)
	}
}

func TestMergeGitHubCommentsKeepsLocal(t *testing.T) {
	t.Parallel()
	existing := []domain.PRComment{
		{ID: "gh-comment-old", Author: "gone", BodyHTML: "stale"},
		{ID: "c-local", Author: "Coder", BodyHTML: "fix pushed"},
	}
	fromGH := []domain.PRComment{
		{ID: "gh-comment-new", Author: "avery", BodyHTML: "fresh"},
	}
	got := mergeGitHubComments(existing, fromGH, false, false)
	if len(got) != 2 {
		t.Fatalf("mergeGitHubComments() len = %d, want 2", len(got))
	}
	if got[0].ID != "gh-comment-new" || got[1].ID != "c-local" {
		t.Errorf("mergeGitHubComments() = %+v, want GH then local", got)
	}
}

func TestMergeGitHubCommentsKeepsFreshInline(t *testing.T) {
	t.Parallel()
	existing := []domain.PRComment{
		{ID: "gh-inline-old", Time: "1d", At: "2026-08-01T00:00:00Z"},
		{ID: "gh-inline-new", Time: "now", Path: "auth.go"},
		{ID: "c-local", Author: "Coder"},
	}
	fromGH := []domain.PRComment{{ID: "gh-comment-1"}}
	got := mergeGitHubComments(existing, fromGH, false, false)
	ids := make([]string, len(got))
	for i, c := range got {
		ids[i] = c.ID
	}
	want := []string{"gh-comment-1", "gh-inline-new", "c-local"}
	if strings.Join(ids, ",") != strings.Join(want, ",") {
		t.Errorf("mergeGitHubComments() ids = %v, want %v", ids, want)
	}
}

func TestMergeGitHubCommentsKeepsOlderWhenSnapshotTruncated(t *testing.T) {
	t.Parallel()
	existing := make([]domain.PRComment, 0, 101)
	fromGH := make([]domain.PRComment, 0, 100)
	for i := 1; i <= 101; i++ {
		c := domain.PRComment{ID: fmt.Sprintf("gh-comment-%d", i), BodyMD: fmt.Sprintf("c%d", i)}
		existing = append(existing, c)
		if i > 1 {
			fromGH = append(fromGH, c)
		}
	}
	got := mergeGitHubComments(existing, fromGH, true, false)
	if len(got) != 101 {
		t.Fatalf("mergeGitHubComments(101 existing, last 100, keepComments) len = %d, want 101", len(got))
	}
	ids := make(map[string]bool, len(got))
	for _, c := range got {
		ids[c.ID] = true
	}
	if !ids["gh-comment-1"] {
		t.Errorf("mergeGitHubComments(...) missing gh-comment-1, want oldest row kept when snapshot is truncated")
	}
	dropped := mergeGitHubComments(existing, fromGH, false, false)
	if len(dropped) != 100 {
		t.Fatalf("mergeGitHubComments(..., keepComments=false) len = %d, want 100", len(dropped))
	}
	for _, c := range dropped {
		if c.ID == "gh-comment-1" {
			t.Errorf("mergeGitHubComments(..., keepComments=false) kept gh-comment-1, want oldest dropped")
		}
	}
}

func TestAllowedMergeMethodsFromRepo(t *testing.T) {
	t.Parallel()
	settings, ok := parseGitHubRepoSettings(`{
		"permissions": {"push": true},
		"allow_merge_commit": false,
		"allow_squash_merge": true,
		"allow_rebase_merge": false
	}`)
	if !ok {
		t.Fatal("parseGitHubRepoSettings() ok = false")
	}
	got := allowedMergeMethodsFromRepo(settings)
	if len(got) != 1 || got[0] != "squash" {
		t.Errorf("allowedMergeMethodsFromRepo() = %v, want [squash]", got)
	}
	unknown, ok := parseGitHubRepoSettings(`{"permissions": {"push": true}}`)
	if !ok {
		t.Fatal("parseGitHubRepoSettings(unknown) ok = false")
	}
	if methods := allowedMergeMethodsFromRepo(unknown); methods != nil {
		t.Errorf("allowedMergeMethodsFromRepo(omitted) = %v, want nil", methods)
	}
}

func TestParseGitHubPRViewMergeStatus(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 22, 12, 0, 0, 0, time.UTC)
	raw := `{
		"title": "Draft OAuth",
		"state": "OPEN",
		"reviewDecision": "REVIEW_REQUIRED",
		"mergeable": "CONFLICTING",
		"mergeStateStatus": "DIRTY",
		"isDraft": true,
		"url": "https://github.com/acme/app/pull/9",
		"author": {"login": "loopbot"}
	}`
	snap, err := parseGitHubPRView(raw, now)
	if err != nil {
		t.Fatalf("parseGitHubPRView() error = %v, want nil", err)
	}
	if !snap.IsDraft || snap.Mergeable != "CONFLICTING" || snap.MergeStateStatus != "DIRTY" || snap.ReviewDecision != "REVIEW_REQUIRED" {
		t.Errorf("parseGitHubPRView() merge = draft=%v mergeable=%q status=%q decision=%q",
			snap.IsDraft, snap.Mergeable, snap.MergeStateStatus, snap.ReviewDecision)
	}
}

func TestGithubConversationState(t *testing.T) {
	t.Parallel()
	pr := domain.PR{State: "In review", Group: "you"}
	state, group := githubConversationState(pr, ghPRSnapshot{State: "OPEN", ReviewDecision: "APPROVED"})
	if state != "Approved" || group != "you" {
		t.Errorf("githubConversationState(approved) = %q %q, want Approved you", state, group)
	}
	state, group = githubConversationState(pr, ghPRSnapshot{State: "MERGED"})
	if state != "Merged" || group != "merged" {
		t.Errorf("githubConversationState(merged) = %q %q, want Merged merged", state, group)
	}
	merged := domain.PR{State: "Merged", Group: "merged"}
	state, group = githubConversationState(merged, ghPRSnapshot{State: "OPEN", ReviewDecision: "APPROVED"})
	if state != "Merged" || group != "merged" {
		t.Errorf("githubConversationState(already merged) = %q %q, want Merged merged", state, group)
	}
	closed := domain.PR{State: "Closed", Group: prGroupMerged}
	state, group = githubConversationState(closed, ghPRSnapshot{State: "OPEN"})
	if state != "Open" || group != prGroupOpen {
		t.Errorf("githubConversationState(reopened) = %q %q, want Open %s", state, group, prGroupOpen)
	}
	state, group = githubConversationState(closed, ghPRSnapshot{State: "OPEN", ReviewDecision: "APPROVED"})
	if state != "Approved" || group != prGroupOpen {
		t.Errorf("githubConversationState(reopened approved) = %q %q, want Approved %s", state, group, prGroupOpen)
	}
}

func TestRefreshPRConversationDoesNotUnmergeInFlight(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "inflight-merge", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-inflight", WorkspaceID: ws.ID, Number: 90, Source: domain.SourceGitHub,
		Title: "old", State: "In review", Group: "you",
		Comments: []domain.PRComment{{ID: "c-1", Author: "avery", BodyHTML: "before"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	entered := make(chan struct{})
	release := make(chan struct{})
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		select {
		case <-entered:
		default:
			close(entered)
		}
		<-release
		return ghPRSnapshot{
			Title:          "from gh",
			State:          "OPEN",
			ReviewDecision: "APPROVED",
			Comments:       []domain.PRComment{{ID: "gh-comment-1", Author: "avery", BodyHTML: "please fix"}},
		}, nil
	}

	done := make(chan bool, 1)
	go func() { done <- eng.RefreshPRConversation(pr.ID) }()
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("snapshot was not called")
	}
	eng.markMerged(pr.ID, "")
	close(release)
	select {
	case changed := <-done:
		if !changed {
			t.Fatal("RefreshPRConversation() changed = false, want true")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("RefreshPRConversation did not return")
	}
	got, _ := st.PR(pr.ID)
	if got.State != "Merged" || got.Group != "merged" {
		t.Errorf("after in-flight refresh State=%q Group=%q, want Merged merged", got.State, got.Group)
	}
}

func TestRefreshPRConversationReopensClosedPR(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reopen", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-reopen", WorkspaceID: ws.ID, Number: 21, Source: domain.SourceGitHub,
		Title: "closed then opened", State: "Closed", Group: prGroupMerged,
		Comments: []domain.PRComment{{ID: "c-1", Author: "avery", BodyHTML: "before"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title: "closed then opened",
			State: "OPEN",
			Comments: []domain.PRComment{
				{ID: "c-1", Author: "avery", BodyHTML: "before"},
				{ID: "gh-reopened", Author: "avery", Kind: "action", BodyHTML: "reopened this"},
			},
		}, nil
	}

	if !eng.RefreshPRConversation(pr.ID) {
		t.Fatal("RefreshPRConversation() = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if got.State != "Open" || got.Group != prGroupOpen {
		t.Errorf("RefreshPRConversation() state=%q group=%q, want Open %s", got.State, got.Group, prGroupOpen)
	}
	if len(got.Comments) != 2 || got.Comments[1].ID != "gh-reopened" {
		t.Errorf("RefreshPRConversation() comments = %+v, want reopen timeline", got.Comments)
	}
}

func TestRefreshPRConversationMirrorsGitHub(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "thread", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-20", WorkspaceID: ws.ID, Number: 20, Source: domain.SourceGitHub,
		Title: "old title", State: "In review", Group: "you",
		Comments: []domain.PRComment{{ID: "c-agent", Author: "Coder", BodyHTML: "working"}},
		Commits:  []domain.Commit{{Hash: "3f1d02b", Author: "Josh", AuthorRole: "Author"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title:          "new title",
			State:          "OPEN",
			URL:            "https://github.com/acme/app/pull/20",
			CreatedAt:      "2026-08-01T09:00:00Z",
			UpdatedAt:      "2026-08-21T15:00:00Z",
			ReviewDecision: "CHANGES_REQUESTED",
			Comments: []domain.PRComment{
				{ID: "gh-comment-1", Author: "avery", BodyHTML: "please fix"},
			},
			commitAuthors: map[string]ghViewActor{"3f1d02babcdef": {Login: "mycodecrafting"}},
			commitMeta:    map[string]ghCommitMeta{"3f1d02babcdef": {Verified: true, CheckStatus: "pass"}},
		}, nil
	}

	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if got.Title != "new title" {
		t.Errorf("RefreshPRConversation() title = %q, want new title", got.Title)
	}
	if got.URL != "https://github.com/acme/app/pull/20" {
		t.Errorf("RefreshPRConversation() URL = %q, want GitHub PR page", got.URL)
	}
	if got.State != "Changes requested" {
		t.Errorf("RefreshPRConversation() state = %q, want Changes requested", got.State)
	}
	if len(got.Comments) != 2 || got.Comments[0].ID != "gh-comment-1" || got.Comments[1].ID != "c-agent" {
		t.Errorf("RefreshPRConversation() comments = %+v, want GH + local agent", got.Comments)
	}
	if len(got.Commits) != 1 || got.Commits[0].Author != "mycodecrafting" || !got.Commits[0].Verified || got.Commits[0].CheckStatus != "pass" {
		t.Errorf("RefreshPRConversation() commits = %+v, want mycodecrafting verified pass", got.Commits)
	}
	if got.CreatedAt != "2026-08-01T09:00:00Z" || got.UpdatedAt != "2026-08-21T15:00:00Z" {
		t.Errorf("RefreshPRConversation() times = created %q updated %q, want gh snapshot stamps", got.CreatedAt, got.UpdatedAt)
	}
}

func TestRefreshPRConversationKeepsOlderCommentsWhenTruncated(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "thread", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	existing := make([]domain.PRComment, 0, 101)
	fromGH := make([]domain.PRComment, 0, 100)
	for i := 1; i <= 101; i++ {
		c := domain.PRComment{ID: fmt.Sprintf("gh-comment-%d", i), BodyMD: fmt.Sprintf("c%d", i)}
		existing = append(existing, c)
		if i > 1 {
			fromGH = append(fromGH, c)
		}
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-101", WorkspaceID: ws.ID, Number: 101, Source: domain.SourceGitHub,
		Title: "busy", State: "In review", Group: "you",
		Comments: existing,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{Comments: fromGH, commentsTruncated: true}, nil
	}

	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if len(got.Comments) != 101 {
		t.Fatalf("RefreshPRConversation() comments len = %d, want 101", len(got.Comments))
	}
	found := false
	for _, c := range got.Comments {
		if c.ID == "gh-comment-1" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("RefreshPRConversation() missing gh-comment-1, want oldest row kept when snapshot is truncated")
	}
}

func TestRefreshPRConversationStoresReviewRequest(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "thread", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-21", WorkspaceID: ws.ID, Number: 21, Source: domain.SourceGitHub,
		Title: "old", State: "Open", Group: "open", AuthorLogin: "sam", ViewerLogin: "avery",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title:          "Please review",
			State:          "OPEN",
			URL:            "https://github.com/acme/app/pull/21",
			AuthorLogin:    "sam",
			ViewerLogin:    "avery",
			ReviewDecision: "REVIEW_REQUIRED",
			ReviewRequests: []ghListedActor{{Login: "avery"}},
		}, nil
	}

	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if !got.ReviewRequested {
		t.Error("RefreshPRConversation() ReviewRequested = false, want true")
	}
	if got.Group != "you" {
		t.Errorf("RefreshPRConversation() group = %q, want you", got.Group)
	}
	if len(got.AttentionReasons) != 1 || got.AttentionReasons[0] != domain.AttentionReviewRequested {
		t.Errorf("RefreshPRConversation() AttentionReasons = %v, want [review_requested]", got.AttentionReasons)
	}
}

func TestPRConversationMinIntervalLetsOpenPRRefresh(t *testing.T) {
	if prConversationMinInterval > 20*time.Second {
		t.Fatalf("prConversationMinInterval = %s; the open PR would sit stale that long", prConversationMinInterval)
	}
}

func TestScheduleRefreshPRConversationDoesNotBlockAndSingleFlights(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "sched-conv", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-sched", WorkspaceID: ws.ID, Number: 21, Source: domain.SourceGitHub,
		Title: "old", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })

	entered := make(chan struct{})
	release := make(chan struct{})
	var calls atomic.Int32
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		calls.Add(1)
		select {
		case <-entered:
		default:
			close(entered)
		}
		<-release
		return ghPRSnapshot{Title: "from gh", State: "OPEN"}, nil
	}

	started := time.Now()
	eng.ScheduleRefreshPRConversation(pr.ID)
	eng.ScheduleRefreshPRConversation(pr.ID)
	if time.Since(started) > 200*time.Millisecond {
		t.Fatal("ScheduleRefreshPRConversation blocked on snapshot")
	}

	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("snapshot was not called")
	}
	eng.ScheduleRefreshPRConversation(pr.ID)
	if n := calls.Load(); n != 1 {
		t.Errorf("in-flight snapshot calls = %d, want 1", n)
	}
	close(release)

	deadline := time.Now().Add(2 * time.Second)
	for {
		got, _ := st.PR(pr.ID)
		if got.Title == "from gh" {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("title = %q, want from gh", got.Title)
		}
		time.Sleep(10 * time.Millisecond)
	}

	eng.ScheduleRefreshPRConversation(pr.ID)
	time.Sleep(50 * time.Millisecond)
	if n := calls.Load(); n != 1 {
		t.Errorf("throttled snapshot calls = %d, want 1", n)
	}
}

func TestRefreshPRConversationStoresMergeStatus(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "merge-status", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-merge", WorkspaceID: ws.ID, Number: 33, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title:            "pr",
			State:            "OPEN",
			ReviewDecision:   "APPROVED",
			Mergeable:        "MERGEABLE",
			MergeStateStatus: "BLOCKED",
			IsDraft:          true,
			HeadRefOid:       "abc123def",
		}, nil
	}

	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if got.Mergeable != "MERGEABLE" || got.MergeStateStatus != "BLOCKED" || got.ReviewDecision != "APPROVED" || !got.Draft || got.HeadSHA != "abc123def" {
		t.Errorf("RefreshPRConversation() merge fields = %+v", got)
	}
}

func TestRefreshPRConversationPreservesMergedUpdatedAt(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "merged-at", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	const frozen = "2026-08-01T12:00:00Z"
	const mergedAt = "2026-08-20T09:00:00Z"
	pr, err := st.AddPR(domain.PR{
		ID: "gh-merged-at", WorkspaceID: ws.ID, Number: 77, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
		CreatedAt: frozen, UpdatedAt: frozen,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title:     "pr",
			State:     "MERGED",
			CreatedAt: frozen,
			UpdatedAt: mergedAt,
		}, nil
	}

	if !eng.RefreshPRConversation(pr.ID) {
		t.Fatal("RefreshPRConversation() = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if got.State != "Merged" || got.Group != "merged" {
		t.Errorf("RefreshPRConversation() state=%q group=%q, want Merged/merged", got.State, got.Group)
	}
	if got.UpdatedAt != mergedAt {
		t.Errorf("RefreshPRConversation() UpdatedAt = %q, want snapshot merge time %q", got.UpdatedAt, mergedAt)
	}
}

func TestRefreshPRConversationStoresAllowedMergeMethods(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "merge-methods", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-methods", WorkspaceID: ws.ID, Number: 55, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title: "pr", State: "OPEN",
			AllowedMergeMethods: []string{"squash", "rebase"},
		}, nil
	}

	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if len(got.AllowedMergeMethods) != 2 || got.AllowedMergeMethods[0] != "squash" || got.AllowedMergeMethods[1] != "rebase" {
		t.Errorf("RefreshPRConversation() allowedMergeMethods = %v, want [squash rebase]", got.AllowedMergeMethods)
	}
}

func TestReadyPRMarksDraftReady(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "ready", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-ready", WorkspaceID: ws.ID, Number: 44, Source: domain.SourceGitHub,
		Title: "draft", State: "In review", Group: "you", Draft: true,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	called := false
	prevReady := setGitHubPRDraftFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		setGitHubPRDraftFn = prevReady
		getGitHubPRSnapshot = prevSnap
	})
	setGitHubPRDraftFn = func(_ context.Context, _ string, _ int, draft bool) error {
		if draft {
			t.Error("ReadyPR() called setGitHubPRDraft(draft=true), want false")
		}
		called = true
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title: "draft", State: "OPEN",
			Mergeable: "MERGEABLE", MergeStateStatus: "CLEAN",
		}, nil
	}

	got, ok := eng.ReadyPR(pr.ID)
	if !ok || !called {
		t.Fatalf("ReadyPR() ok=%v called=%v, want true true", ok, called)
	}
	if got.Draft {
		t.Errorf("ReadyPR() draft = true, want false")
	}
}

func TestSetPRDraftConvertsToDraft(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "to-draft", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-to-draft", WorkspaceID: ws.ID, Number: 45, Source: domain.SourceGitHub,
		Title: "ready", State: "In review", Group: "you", Draft: false,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	var gotDraft *bool
	prevReady := setGitHubPRDraftFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		setGitHubPRDraftFn = prevReady
		getGitHubPRSnapshot = prevSnap
	})
	setGitHubPRDraftFn = func(_ context.Context, _ string, _ int, draft bool) error {
		gotDraft = &draft
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{Title: "ready", State: "OPEN", IsDraft: true}, nil
	}

	got, ok := eng.SetPRDraft(pr.ID, true)
	if !ok || gotDraft == nil || !*gotDraft {
		t.Fatalf("SetPRDraft(true) ok=%v draft=%v, want true true", ok, gotDraft)
	}
	if !got.Draft {
		t.Errorf("SetPRDraft(true) Draft = false, want true")
	}
}

func TestSetPRDraftLocalPersistsFlag(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "local-draft", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "local-draft", WorkspaceID: ws.ID, Source: domain.SourceLocal,
		Title: "local", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	called := false
	prev := setGitHubPRDraftFn
	t.Cleanup(func() { setGitHubPRDraftFn = prev })
	setGitHubPRDraftFn = func(context.Context, string, int, bool) error {
		called = true
		return nil
	}
	got, ok := eng.SetPRDraft(pr.ID, true)
	if !ok || !got.Draft || called {
		t.Fatalf("SetPRDraft(local) ok=%v draft=%v calledGH=%v", ok, got.Draft, called)
	}
}

func TestAddPRCommentGitHubPosts(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "post", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-21", WorkspaceID: ws.ID, Number: 21, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	posted := false
	var reviewComments []domain.PendingLineComment
	prevPost := postGitHubPRCommentFn
	prevRev := submitGitHubPRReviewFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		postGitHubPRCommentFn = prevPost
		submitGitHubPRReviewFn = prevRev
		getGitHubPRSnapshot = prevSnap
	})
	postGitHubPRCommentFn = func(context.Context, string, int, string, int, string) error {
		posted = true
		return nil
	}
	submitGitHubPRReviewFn = func(_ context.Context, _ string, _ int, _, _ string, comments []domain.PendingLineComment) error {
		reviewComments = comments
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			State: "OPEN",
			Comments: []domain.PRComment{
				{ID: "gh-inline-1", Author: "you", BodyHTML: "nits"},
			},
		}, nil
	}

	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, ""); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}
	if posted {
		t.Error("line note posted to GitHub immediately, want pending until SubmitReview")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 1 || got.PendingComments[0].File != "auth.go" || got.PendingComments[0].Line != 12 {
		t.Errorf("AddPRComment() pending = %+v, want auth.go:12", got.PendingComments)
	}
	if len(got.Comments) != 0 {
		t.Errorf("AddPRComment() thread = %+v, want empty until submit", got.Comments)
	}
	if _, ok := eng.SubmitReview(pr.ID, "comment", "nits wrap"); !ok {
		t.Fatal("SubmitReview() ok = false")
	}
	if len(reviewComments) != 1 || reviewComments[0].Body != "nits" {
		t.Errorf("SubmitReview() comments = %+v, want nits", reviewComments)
	}
}

func TestAddPRCommentMultilinePending(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "multi", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-22", WorkspaceID: ws.ID, Number: 22, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	var reviewComments []domain.PendingLineComment
	prevRev := submitGitHubPRReviewFn
	t.Cleanup(func() { submitGitHubPRReviewFn = prevRev })
	submitGitHubPRReviewFn = func(_ context.Context, _ string, _ int, _, _ string, comments []domain.PendingLineComment) error {
		reviewComments = comments
		return nil
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "span", 10, ""); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 1 {
		t.Fatalf("AddPRComment() pending len = %d, want 1", len(got.PendingComments))
	}
	pc := got.PendingComments[0]
	if pc.File != "auth.go" || pc.Line != 12 || pc.StartLine != 10 || pc.Side != "RIGHT" || pc.StartSide != "RIGHT" {
		t.Errorf("AddPRComment() pending = %+v, want auth.go:10-12 RIGHT", pc)
	}
	if _, ok := eng.SubmitReview(pr.ID, "comment", "wrap"); !ok {
		t.Fatal("SubmitReview() ok = false")
	}
	if len(reviewComments) != 1 || reviewComments[0].StartLine != 10 || reviewComments[0].Line != 12 {
		t.Errorf("SubmitReview() comments = %+v, want start 10 line 12", reviewComments)
	}
}

func TestAddPRCommentPersistsFindingID(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "finding-id", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-find", WorkspaceID: ws.ID, Number: 23, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, "F-1"); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 1 || got.PendingComments[0].FindingID != "F-1" {
		t.Errorf("AddPRComment() findingId = %+v, want F-1", got.PendingComments)
	}
}

func TestAddPRCommentDeleteRemovesOnlyMatchingDraft(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "two-drafts", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-two", WorkspaceID: ws.ID, Number: 24, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "first", 0, "F-1"); !ok {
		t.Fatal("AddPRComment() F-1 ok = false, want true")
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "second", 0, "F-2"); !ok {
		t.Fatal("AddPRComment() F-2 ok = false, want true")
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "", 0, "F-1"); !ok {
		t.Fatal("AddPRComment() delete F-1 ok = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 1 || got.PendingComments[0].FindingID != "F-2" {
		t.Errorf("AddPRComment() delete pending = %+v, want only F-2", got.PendingComments)
	}
}

func TestSubmitReviewResolvesPendingFindings(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resolve-on-submit", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-1", WorkspaceID: ws.ID, File: "auth.go", Line: 12,
		Severity: domain.SevHigh, Message: "nits", Status: string(domain.FindingOpen),
	})
	pr, err := st.AddPR(domain.PR{
		ID: "gh-resolve", WorkspaceID: ws.ID, Number: 25, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, "F-1"); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}

	prevRev := submitGitHubPRReviewFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		submitGitHubPRReviewFn = prevRev
		getGitHubPRSnapshot = prevSnap
	})
	submitGitHubPRReviewFn = func(context.Context, string, int, string, string, []domain.PendingLineComment) error {
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{State: "OPEN", Title: "pr"}, nil
	}

	if _, ok := eng.SubmitReview(pr.ID, "comment", "wrap"); !ok {
		t.Fatal("SubmitReview() ok = false, want true")
	}
	findings := st.Findings(ws.ID)
	if len(findings) != 1 || findings[0].Status != string(domain.FindingResolved) {
		t.Errorf("SubmitReview() findings = %+v, want F-1 resolved", findings)
	}
	var saw bool
	for _, ev := range sink.byName("findings.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID {
			saw = true
		}
	}
	if !saw {
		t.Error("SubmitReview() findings.updated = missing, want workspace event")
	}
}

func TestSendBackToLoopResolvesPendingFindings(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resolve-on-sendback", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-sb", WorkspaceID: ws.ID, File: "auth.go", Line: 12,
		Severity: domain.SevHigh, Message: "nits", Status: string(domain.FindingOpen),
	})
	pr, err := st.AddPR(domain.PR{
		ID: "gh-sendback-resolve", WorkspaceID: ws.ID, Number: 26, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, "F-sb"); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}

	prevRev := submitGitHubPRReviewFn
	t.Cleanup(func() { submitGitHubPRReviewFn = prevRev })
	submitGitHubPRReviewFn = func(context.Context, string, int, string, string, []domain.PendingLineComment) error {
		return nil
	}

	if _, ok := eng.SendBackToLoop(pr.ID, []string{"fix it"}); !ok {
		t.Fatal("SendBackToLoop() ok = false, want true")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 0 {
		t.Errorf("SendBackToLoop() pending = %+v, want flushed", got.PendingComments)
	}
	findings := st.Findings(ws.ID)
	if len(findings) != 1 || findings[0].Status != string(domain.FindingResolved) {
		t.Errorf("SendBackToLoop() findings = %+v, want F-sb resolved", findings)
	}
	if findings[0].UpdatedBy != domain.FindingActorEngine {
		t.Errorf("SendBackToLoop() UpdatedBy = %q, want %q", findings[0].UpdatedBy, domain.FindingActorEngine)
	}
	var saw bool
	for _, ev := range sink.byName("findings.updated") {
		d, _ := ev.Data.(map[string]any)
		if d["workspaceId"] == ws.ID {
			saw = true
		}
	}
	if !saw {
		t.Error("SendBackToLoop() findings.updated = missing, want workspace event")
	}
}

func TestSendBackToLoopPreservesPendingOnGitHubFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "preserve-on-sendback-fail", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-sb-fail", WorkspaceID: ws.ID, File: "auth.go", Line: 12,
		Severity: domain.SevHigh, Message: "nits", Status: string(domain.FindingOpen),
	})
	pr, err := st.AddPR(domain.PR{
		ID: "gh-sendback-fail", WorkspaceID: ws.ID, Number: 27, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, "F-sb-fail"); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}

	prevRev := submitGitHubPRReviewFn
	t.Cleanup(func() { submitGitHubPRReviewFn = prevRev })
	submitGitHubPRReviewFn = func(context.Context, string, int, string, string, []domain.PendingLineComment) error {
		return fmt.Errorf("network down")
	}

	if _, ok := eng.SendBackToLoop(pr.ID, []string{"fix it"}); ok {
		t.Fatal("SendBackToLoop() ok = true, want false on GitHub error")
	}
	got, _ := st.PR(pr.ID)
	if len(got.PendingComments) != 1 || got.PendingComments[0].FindingID != "F-sb-fail" {
		t.Errorf("SendBackToLoop() pending = %+v, want F-sb-fail preserved", got.PendingComments)
	}
	findings := st.Findings(ws.ID)
	if len(findings) != 1 || findings[0].Status != string(domain.FindingOpen) {
		t.Errorf("SendBackToLoop() findings = %+v, want F-sb-fail still open", findings)
	}
}

func TestSendBackToLoopResolvesLocalPendingFindings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	ws := domain.Workspace{
		ID: "L-sb-find", ProjectID: "P", Goal: "Add OAuth",
		State: domain.StatePROpen, RunStarted: true, LoopID: "recur.loops.pr",
	}
	if _, err := st.AddWorkspace(ws); err != nil {
		t.Fatalf("AddWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-local", WorkspaceID: ws.ID, File: "auth.go", Line: 8,
		Severity: domain.SevMedium, Message: "unwrap", Status: string(domain.FindingOpen),
	})
	pr, err := st.AddPR(domain.PR{
		ID: "pr-sb-find", WorkspaceID: ws.ID, Number: 12, Title: "Add OAuth",
		State: "In review", Group: "you",
		PendingComments: []domain.PendingLineComment{
			{File: "auth.go", Line: 8, Body: "unwrap this error", FindingID: "F-local"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	if _, ok := eng.SendBackToLoop(pr.ID, []string{"fix the unwrap"}); !ok {
		t.Fatal("SendBackToLoop() ok = false, want true")
	}
	findings := st.Findings(ws.ID)
	if len(findings) != 1 || findings[0].Status != string(domain.FindingResolved) {
		t.Errorf("SendBackToLoop() findings = %+v, want F-local resolved", findings)
	}
}

func TestSubmitReviewLeavesDismissedPendingFindings(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "skip-dismissed", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	st.AddFinding(domain.Finding{
		ID: "F-ig", WorkspaceID: ws.ID, File: "auth.go", Line: 12,
		Severity: domain.SevHigh, Message: "ignored", Status: string(domain.FindingOpen),
	})
	st.AddFinding(domain.Finding{
		ID: "F-inv", WorkspaceID: ws.ID, File: "auth.go", Line: 20,
		Severity: domain.SevLow, Message: "invalid", Status: string(domain.FindingOpen),
	})
	st.AddFinding(domain.Finding{
		ID: "F-open", WorkspaceID: ws.ID, File: "auth.go", Line: 30,
		Severity: domain.SevMedium, Message: "open", Status: string(domain.FindingOpen),
	})
	if _, err := st.SetFindingStatus(ws.ID, "F-ig", string(domain.FindingIgnored)); err != nil {
		t.Fatalf("SetFindingStatus ignore: %v", err)
	}
	if _, err := st.SetFindingStatus(ws.ID, "F-inv", string(domain.FindingInvalid)); err != nil {
		t.Fatalf("SetFindingStatus invalid: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-dismiss", WorkspaceID: ws.ID, Number: 27, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
		PendingComments: []domain.PendingLineComment{
			{File: "auth.go", Line: 12, Body: "ignored draft", FindingID: "F-ig"},
			{File: "auth.go", Line: 20, Body: "invalid draft", FindingID: "F-inv"},
			{File: "auth.go", Line: 30, Body: "open draft", FindingID: "F-open"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	prevRev := submitGitHubPRReviewFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		submitGitHubPRReviewFn = prevRev
		getGitHubPRSnapshot = prevSnap
	})
	submitGitHubPRReviewFn = func(context.Context, string, int, string, string, []domain.PendingLineComment) error {
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{State: "OPEN", Title: "pr"}, nil
	}

	if _, ok := eng.SubmitReview(pr.ID, "comment", "wrap"); !ok {
		t.Fatal("SubmitReview() ok = false, want true")
	}
	byID := map[string]string{}
	for _, f := range st.Findings(ws.ID) {
		byID[f.ID] = f.Status
	}
	if byID["F-ig"] != string(domain.FindingIgnored) || byID["F-inv"] != string(domain.FindingInvalid) {
		t.Errorf("SubmitReview() dismissed = %v, want F-ig ignored F-inv invalid", byID)
	}
	if byID["F-open"] != string(domain.FindingResolved) {
		t.Errorf("SubmitReview() open = %q, want resolved", byID["F-open"])
	}
}

func TestAddPRCommentPendingOnlyEvent(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "pend", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "pend-1", WorkspaceID: ws.ID, Number: 1, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "auth.go", 12, "nits", 0, ""); !ok {
		t.Fatal("AddPRComment() ok = false, want true")
	}
	var got map[string]any
	for _, ev := range sink.byName("pr.updated") {
		d, ok := ev.Data.(map[string]any)
		if !ok || d["prId"] != pr.ID {
			continue
		}
		got = d
	}
	if got == nil {
		t.Fatal("AddPRComment() pr.updated = nil, want pending-only event")
	}
	if got["pendingOnly"] != true {
		t.Errorf("AddPRComment() pendingOnly = %v, want true", got["pendingOnly"])
	}
	comments, _ := got["pendingComments"].([]domain.PendingLineComment)
	if len(comments) != 1 || comments[0].File != "auth.go" || comments[0].Line != 12 {
		t.Errorf("AddPRComment() pendingComments = %+v, want auth.go:12", got["pendingComments"])
	}
}

func TestSubmitReviewGitHubPosts(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "review", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-22", WorkspaceID: ws.ID, Number: 22, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}

	var kind, body string
	prevRev := submitGitHubPRReviewFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		submitGitHubPRReviewFn = prevRev
		getGitHubPRSnapshot = prevSnap
	})
	submitGitHubPRReviewFn = func(_ context.Context, _ string, _ int, k, b string, _ []domain.PendingLineComment) error {
		kind, body = k, b
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{State: "OPEN", ReviewDecision: "APPROVED", Title: "pr"}, nil
	}

	updated, ok := eng.SubmitReview(pr.ID, "approve", "ship it")
	if !ok {
		t.Fatal("SubmitReview() ok = false, want true")
	}
	if kind != "approve" || body != "ship it" {
		t.Errorf("submitGitHubPRReviewFn() = %q %q, want approve ship it", kind, body)
	}
	if updated.State != "Approved" {
		t.Errorf("SubmitReview() state = %q, want Approved", updated.State)
	}
}

func TestSubmitReviewGitHubRejectsOwnPR(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "own-review", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-23", WorkspaceID: ws.ID, Number: 23, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
		AuthorLogin: "avery", ViewerLogin: "avery", ViewerCanWrite: true,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	called := false
	prev := submitGitHubPRReviewFn
	t.Cleanup(func() { submitGitHubPRReviewFn = prev })
	submitGitHubPRReviewFn = func(context.Context, string, int, string, string, []domain.PendingLineComment) error {
		called = true
		return nil
	}
	if _, ok := eng.SubmitReview(pr.ID, "approve", "ship it"); ok {
		t.Fatal("SubmitReview() ok = true, want false on own PR")
	}
	if called {
		t.Error("submitGitHubPRReviewFn called, want skip for own PR")
	}
}

func TestAddPRCommentGitHubFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "fail", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-23", WorkspaceID: ws.ID, Number: 23, Source: domain.SourceGitHub,
		Title: "pr", State: "In review", Group: "you",
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prev := postGitHubPRCommentFn
	t.Cleanup(func() { postGitHubPRCommentFn = prev })
	postGitHubPRCommentFn = func(context.Context, string, int, string, int, string) error {
		return fmt.Errorf("network down")
	}
	if _, ok := eng.AddPRComment(pr.ID, "", 0, "hi", 0, ""); ok {
		t.Error("AddPRComment() ok = true, want false on GitHub error")
	}
}

func TestPRReviewCapabilities(t *testing.T) {
	t.Parallel()
	local := domain.PR{Source: domain.SourceLocal, ViewerLogin: "avery"}
	if !local.CanEditDetails() || !local.CanApproveReview() {
		t.Errorf("local PR CanEditDetails/CanApproveReview = %v %v, want true true", local.CanEditDetails(), local.CanApproveReview())
	}
	if !local.CanEditComment(domain.PRComment{Author: "You"}) {
		t.Error("local PR CanEditComment(You) = false, want true")
	}
	if local.CanEditComment(domain.PRComment{Author: "Coder"}) {
		t.Error("local PR CanEditComment(Coder) = true, want false")
	}

	author := domain.PR{Source: domain.SourceGitHub, AuthorLogin: "avery", ViewerLogin: "avery", ViewerCanWrite: true}
	if !author.CanEditDetails() {
		t.Error("author CanEditDetails() = false, want true")
	}
	if author.CanApproveReview() {
		t.Error("author CanApproveReview() = true, want false (GitHub forbids self-review)")
	}
	merged := domain.PR{Source: domain.SourceGitHub, State: "Merged", Group: "merged", ViewerCanWrite: true, ViewerLogin: "pat", AuthorLogin: "loopbot"}
	if !merged.ConversationLocked() || merged.CanApproveReview() || merged.CanEditDetails() {
		t.Errorf("merged ConversationLocked/CanApproveReview/CanEditDetails = %v %v %v, want true false false", merged.ConversationLocked(), merged.CanApproveReview(), merged.CanEditDetails())
	}
	closed := domain.PR{Source: domain.SourceLocal, State: "Closed"}
	if !closed.ConversationLocked() || closed.CanEditDetails() {
		t.Errorf("closed ConversationLocked/CanEditDetails = %v %v, want true false", closed.ConversationLocked(), closed.CanEditDetails())
	}
	if !author.CanEditComment(domain.PRComment{AuthorLogin: "avery"}) {
		t.Error("author CanEditComment(own) = false, want true")
	}

	reader := domain.PR{Source: domain.SourceGitHub, AuthorLogin: "loopbot", ViewerLogin: "avery", ViewerCanWrite: false}
	if reader.CanEditDetails() || reader.CanApproveReview() {
		t.Errorf("reader CanEditDetails/CanApproveReview = %v %v, want false false", reader.CanEditDetails(), reader.CanApproveReview())
	}
	if !reader.CanEditComment(domain.PRComment{AuthorLogin: "avery"}) {
		t.Error("reader CanEditComment(own) = false, want true")
	}

	writer := domain.PR{Source: domain.SourceGitHub, AuthorLogin: "loopbot", ViewerLogin: "avery", ViewerCanWrite: true}
	if !writer.CanEditDetails() || !writer.CanApproveReview() {
		t.Errorf("writer CanEditDetails/CanApproveReview = %v %v, want true true", writer.CanEditDetails(), writer.CanApproveReview())
	}

	ownNote := domain.PRComment{ID: "gh-comment-IC_1", AuthorLogin: "avery"}
	if !author.CanDeleteComment(ownNote) {
		t.Error("author CanDeleteComment(own conversation) = false, want true")
	}
	if !writer.CanDeleteComment(domain.PRComment{ID: "gh-comment-IC_2", AuthorLogin: "sam"}) {
		t.Error("writer CanDeleteComment(other conversation) = false, want true")
	}
	if reader.CanDeleteComment(domain.PRComment{ID: "gh-comment-IC_2", AuthorLogin: "sam"}) {
		t.Error("reader CanDeleteComment(other conversation) = true, want false")
	}
	if author.CanDeleteComment(domain.PRComment{ID: "gh-review-PRR_1", AuthorLogin: "avery"}) {
		t.Error("author CanDeleteComment(review) = true, want false")
	}
	if author.CanDeleteComment(domain.PRComment{ID: "gh-description", Kind: "description", AuthorLogin: "avery"}) {
		t.Error("author CanDeleteComment(description) = true, want false")
	}
	if !local.CanDeleteComment(domain.PRComment{ID: "c1", Author: "You"}) {
		t.Error("local CanDeleteComment(You) = false, want true")
	}

	if !author.CanReactComment(domain.PRComment{ID: "gh-comment-IC_1"}) || !reader.CanReactComment(ownNote) {
		t.Error("CanReactComment(authenticated) = false, want true")
	}
	if author.CanReactComment(domain.PRComment{ID: "gh-event-1", Kind: "action", CommitSHA: "abc"}) {
		t.Error("CanReactComment(commit) = true, want false")
	}
	if !local.CanReactComment(domain.PRComment{ID: "c1"}) {
		t.Error("local CanReactComment = false, want true")
	}
}

func TestAddPRCommentLocked(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "locked", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "merged-1", WorkspaceID: ws.ID, Number: 1, Source: domain.SourceGitHub,
		Title: "pr", State: "Merged", Group: "merged",
		Comments: []domain.PRComment{{ID: "gh-inline-1", Kind: "comment", Path: "a.go", Line: 1}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	if _, ok := eng.AddPRComment(pr.ID, "", 0, "too late", 0, ""); ok {
		t.Error("AddPRComment on merged PR ok = true, want false")
	}
	if _, ok := eng.SubmitReview(pr.ID, "approve", "lgtm"); ok {
		t.Error("SubmitReview on merged PR ok = true, want false")
	}
	if _, ok := eng.ReplyPRComment(pr.ID, "gh-inline-1", "too late"); ok {
		t.Error("ReplyPRComment on merged PR ok = true, want false")
	}
	if _, ok := eng.UpdatePRDetails(pr.ID, "nope", "nope"); ok {
		t.Error("UpdatePRDetails on merged PR ok = true, want false")
	}
}

func TestUpdatePRDetailsLocal(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	pr, err := st.AddPR(domain.PR{
		ID: "10", Number: 10, Source: domain.SourceLocal, Title: "old",
		Comments: []domain.PRComment{{ID: "gh-description", Kind: "description", BodyMD: "old body"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, ok := eng.UpdatePRDetails(pr.ID, "new title", "new body")
	if !ok {
		t.Fatal("UpdatePRDetails() ok = false, want true")
	}
	if got.Title != "new title" {
		t.Errorf("UpdatePRDetails() title = %q, want new title", got.Title)
	}
	if len(got.Comments) == 0 || got.Comments[0].BodyMD != "new body" {
		t.Errorf("UpdatePRDetails() comments = %+v, want description new body", got.Comments)
	}
}

func TestUpdatePRCommentLocal(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	pr, err := st.AddPR(domain.PR{
		ID: "11", Number: 11, Source: domain.SourceLocal, Title: "pr",
		Comments: []domain.PRComment{{ID: "c-you", Author: "You", BodyHTML: "old", BodyMD: "old"}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	got, ok := eng.UpdatePRComment(pr.ID, "c-you", "edited")
	if !ok {
		t.Fatal("UpdatePRComment() ok = false, want true")
	}
	if got.BodyMD != "edited" {
		t.Errorf("UpdatePRComment() BodyMD = %q, want edited", got.BodyMD)
	}
	if _, ok := eng.UpdatePRComment(pr.ID, "missing", "x"); ok {
		t.Error("UpdatePRComment(missing) ok = true, want false")
	}
}

func TestUpdatePRDetailsGitHub(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "edit", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-24", WorkspaceID: ws.ID, Number: 24, Source: domain.SourceGitHub,
		Title: "old", AuthorLogin: "avery", ViewerLogin: "avery", ViewerCanWrite: true,
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	var editedTitle, editedBody string
	prevEdit := editGitHubPRFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		editGitHubPRFn = prevEdit
		getGitHubPRSnapshot = prevSnap
	})
	editGitHubPRFn = func(_ context.Context, _ string, n int, title, body string) error {
		if n != 24 {
			t.Errorf("editGitHubPRFn number = %d, want 24", n)
		}
		editedTitle, editedBody = title, body
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			Title: editedTitle, AuthorLogin: "avery", ViewerLogin: "avery", ViewerCanWrite: true,
			Comments: []domain.PRComment{{ID: "gh-description", Kind: "description", BodyMD: editedBody, AuthorLogin: "avery"}},
		}, nil
	}
	got, ok := eng.UpdatePRDetails(pr.ID, "ship it", "please review")
	if !ok {
		t.Fatal("UpdatePRDetails() ok = false, want true")
	}
	if editedTitle != "ship it" || editedBody != "please review" {
		t.Errorf("editGitHubPRFn title/body = %q %q, want ship it / please review", editedTitle, editedBody)
	}
	if got.Title != "ship it" {
		t.Errorf("UpdatePRDetails() title = %q, want ship it", got.Title)
	}

	denied, err := st.AddPR(domain.PR{
		ID: "gh-25", WorkspaceID: ws.ID, Number: 25, Source: domain.SourceGitHub,
		Title: "other", AuthorLogin: "loopbot", ViewerLogin: "avery", ViewerCanWrite: false,
	})
	if err != nil {
		t.Fatalf("AddPR denied: %v", err)
	}
	if _, ok := eng.UpdatePRDetails(denied.ID, "nope", "nope"); ok {
		t.Error("UpdatePRDetails() ok = true for reader, want false")
	}
}

func TestUpdatePRCommentGitHub(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "cedit", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-26", WorkspaceID: ws.ID, Number: 26, Source: domain.SourceGitHub,
		ViewerLogin: "avery",
		Comments: []domain.PRComment{
			{ID: "gh-comment-IC_1", Author: "avery", AuthorLogin: "avery", BodyMD: "old"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	var editedID, editedBody string
	prevEdit := editGitHubPRCommentFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		editGitHubPRCommentFn = prevEdit
		getGitHubPRSnapshot = prevSnap
	})
	editGitHubPRCommentFn = func(_ context.Context, _ string, _ int, id, body string) error {
		editedID, editedBody = id, body
		return nil
	}
	refreshed := false
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		refreshed = true
		return ghPRSnapshot{}, fmt.Errorf("should not refresh")
	}
	got, ok := eng.UpdatePRComment(pr.ID, "gh-comment-IC_1", "fixed")
	if !ok {
		t.Fatal("UpdatePRComment() ok = false, want true")
	}
	if refreshed {
		t.Error("UpdatePRComment() called RefreshPRConversation, want local persist")
	}
	if editedID != "gh-comment-IC_1" || editedBody != "fixed" {
		t.Errorf("editGitHubPRCommentFn id/body = %q %q, want gh-comment-IC_1 / fixed", editedID, editedBody)
	}
	if got.BodyMD != "fixed" {
		t.Errorf("UpdatePRComment() BodyMD = %q, want fixed", got.BodyMD)
	}
}

func TestReplyGitHubPRCommentSkipsRefresh(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "reply", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-27", WorkspaceID: ws.ID, Number: 27, Source: domain.SourceGitHub,
		URL: "https://github.com/acme/app/pull/27", ViewerLogin: "avery",
		Comments: []domain.PRComment{{
			ID: "gh-inline-1", ParentID: "gh-review-9", Path: "auth.go", Line: 12,
			ThreadID: "PRRT_1", BodyMD: "hmm",
		}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevReply := replyGitHubPRCommentFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		replyGitHubPRCommentFn = prevReply
		getGitHubPRSnapshot = prevSnap
	})
	refreshed := false
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		refreshed = true
		return ghPRSnapshot{}, fmt.Errorf("should not refresh")
	}
	replyGitHubPRCommentFn = func(_ context.Context, _ string, _ int, _, body string) (string, error) {
		return fmt.Sprintf(`{"id":999,"body":%q,"path":"auth.go","line":12,"in_reply_to_id":1,"created_at":"2026-08-21T12:00:00Z","user":{"login":"avery"}}`, body), nil
	}
	got, ok := eng.ReplyPRComment(pr.ID, "gh-inline-1", "thread reply")
	if !ok {
		t.Fatal("ReplyPRComment() ok = false, want true")
	}
	if refreshed {
		t.Error("ReplyPRComment() called RefreshPRConversation, want local persist")
	}
	if got.ID != "gh-inline-999" || got.InReplyToID != "gh-inline-1" || got.ParentID != "gh-review-9" || got.ThreadID != "PRRT_1" {
		t.Errorf("ReplyPRComment() = %+v, want gh-inline-999 under gh-review-9 / PRRT_1", got)
	}
	stored, _ := st.PR(pr.ID)
	found := false
	for _, c := range stored.Comments {
		if c.ID == "gh-inline-999" {
			found = true
		}
	}
	if !found {
		t.Errorf("ReplyPRComment() store comments = %+v, want gh-inline-999", stored.Comments)
	}
	if !sink.has("pr.comment") {
		t.Error("ReplyPRComment() events missing pr.comment")
	}
}

func TestDropRememberedComments(t *testing.T) {
	t.Parallel()
	eng := &Engine{}
	eng.rememberDroppedComments("p1", []string{"gh-inline-1", "gh-inline-2"})
	got := eng.dropRememberedComments("p1", []domain.PRComment{
		{ID: "gh-inline-1"},
		{ID: "gh-inline-3"},
	})
	if len(got) != 1 || got[0].ID != "gh-inline-3" {
		t.Errorf("dropRememberedComments() = %+v, want only gh-inline-3", got)
	}
}

func TestDeleteGitHubPRCommentSkipsRefresh(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "del", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-28", WorkspaceID: ws.ID, Number: 28, Source: domain.SourceGitHub,
		ViewerLogin: "avery", ViewerCanWrite: true,
		Comments: []domain.PRComment{
			{ID: "gh-inline-1", AuthorLogin: "avery", Path: "auth.go", Line: 12, BodyMD: "hmm"},
			{ID: "gh-inline-2", InReplyToID: "gh-inline-1", Path: "auth.go", Line: 12, BodyMD: "reply"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevDel := deleteGitHubPRCommentFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		deleteGitHubPRCommentFn = prevDel
		getGitHubPRSnapshot = prevSnap
	})
	refreshed := false
	deletedID := ""
	deleteGitHubPRCommentFn = func(_ context.Context, _ string, _ int, id string) error {
		deletedID = id
		return nil
	}
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		refreshed = true
		return ghPRSnapshot{}, fmt.Errorf("should not refresh")
	}
	got, ok := eng.DeletePRComment(pr.ID, "gh-inline-1")
	if !ok {
		t.Fatal("DeletePRComment() ok = false, want true")
	}
	if refreshed {
		t.Error("DeletePRComment() called RefreshPRConversation, want local persist")
	}
	if deletedID != "gh-inline-1" {
		t.Errorf("deleteGitHubPRCommentFn id = %q, want gh-inline-1", deletedID)
	}
	for _, c := range got.Comments {
		if c.ID == "gh-inline-1" || c.InReplyToID == "gh-inline-1" {
			t.Errorf("DeletePRComment() leftover comment %+v, want root and replies gone", c)
		}
	}
	revived := mergeGitHubComments(got.Comments, []domain.PRComment{
		{ID: "gh-inline-1", Path: "auth.go", BodyMD: "hmm"},
	}, false, false)
	held := eng.dropRememberedComments(pr.ID, revived)
	for _, c := range held {
		if c.ID == "gh-inline-1" {
			t.Errorf("dropRememberedComments() kept %+v after delete, want it hidden from refresh", c)
		}
	}
}

func TestDeleteGitHubConversationComment(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delconv", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-30", WorkspaceID: ws.ID, Number: 30, Source: domain.SourceGitHub,
		ViewerLogin: "avery", ViewerCanWrite: true,
		Comments: []domain.PRComment{
			{ID: "gh-comment-IC_1", AuthorLogin: "avery", BodyMD: "looks good"},
			{ID: "gh-review-PRR_1", AuthorLogin: "avery", BodyMD: "ship it"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevDel := deleteGitHubPRCommentFn
	t.Cleanup(func() { deleteGitHubPRCommentFn = prevDel })
	deletedID := ""
	deleteGitHubPRCommentFn = func(_ context.Context, _ string, _ int, id string) error {
		deletedID = id
		return nil
	}
	got, ok := eng.DeletePRComment(pr.ID, "gh-comment-IC_1")
	if !ok {
		t.Fatal("DeletePRComment() conversation ok = false, want true")
	}
	if deletedID != "gh-comment-IC_1" {
		t.Errorf("deleteGitHubPRCommentFn id = %q, want gh-comment-IC_1", deletedID)
	}
	for _, c := range got.Comments {
		if c.ID == "gh-comment-IC_1" {
			t.Error("DeletePRComment() left the conversation comment")
		}
	}
	if _, ok := eng.DeletePRComment(pr.ID, "gh-review-PRR_1"); ok {
		t.Error("DeletePRComment() review ok = true, want false")
	}
}

func TestDeleteGitHubPRCommentRestoresOnFailure(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "delfail", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-29", WorkspaceID: ws.ID, Number: 29, Source: domain.SourceGitHub,
		ViewerLogin: "avery", ViewerCanWrite: true,
		CreatedAt: "2026-08-01T12:00:00Z", UpdatedAt: "2026-08-01T12:00:00Z",
		Comments: []domain.PRComment{
			{ID: "gh-inline-1", AuthorLogin: "avery", Path: "auth.go", Line: 12, BodyMD: "hmm"},
		},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevDel := deleteGitHubPRCommentFn
	t.Cleanup(func() { deleteGitHubPRCommentFn = prevDel })
	deleteGitHubPRCommentFn = func(context.Context, string, int, string) error {
		return fmt.Errorf("denied")
	}
	if _, ok := eng.DeletePRComment(pr.ID, "gh-inline-1"); ok {
		t.Error("DeletePRComment() ok = true, want false on GitHub error")
	}
	stored, _ := st.PR(pr.ID)
	found := false
	for _, c := range stored.Comments {
		if c.ID == "gh-inline-1" {
			found = true
		}
	}
	if !found {
		t.Errorf("DeletePRComment() store comments = %+v, want gh-inline-1 restored", stored.Comments)
	}
	if stored.UpdatedAt != "2026-08-01T12:00:00Z" {
		t.Errorf("DeletePRComment() UpdatedAt = %q, want restored pre-delete stamp", stored.UpdatedAt)
	}
}

func TestApplyRememberedThreadResolved(t *testing.T) {
	t.Parallel()
	eng := &Engine{}
	eng.rememberThreadResolved("p1", "PRRT_1", true)
	got := eng.applyRememberedThreadResolved("p1", []domain.PRComment{
		{ID: "gh-inline-1", ThreadID: "PRRT_1", Resolved: false},
		{ID: "gh-inline-2", ThreadID: "PRRT_2", Resolved: false},
	})
	if !got[0].Resolved || got[1].Resolved {
		t.Errorf("applyRememberedThreadResolved() = %+v, want PRRT_1 resolved and PRRT_2 open", got)
	}
}

func TestSetGitHubThreadResolvedSkipsRefresh(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "resolve", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-29", WorkspaceID: ws.ID, Number: 29, Source: domain.SourceGitHub,
		ViewerLogin: "avery", ViewerCanWrite: true,
		Comments: []domain.PRComment{{
			ID: "gh-inline-1", Path: "auth.go", Line: 12, ThreadID: "PRRT_1", BodyMD: "hmm",
		}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevSet := setGitHubReviewThreadFn
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() {
		setGitHubReviewThreadFn = prevSet
		getGitHubPRSnapshot = prevSnap
	})
	refreshed := false
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		refreshed = true
		return ghPRSnapshot{}, fmt.Errorf("should not refresh")
	}
	setGitHubReviewThreadFn = func(_ context.Context, _, threadID string, resolved bool) error {
		if threadID != "PRRT_1" || !resolved {
			t.Errorf("setGitHubReviewThreadFn(%q, %v), want PRRT_1 true", threadID, resolved)
		}
		return nil
	}
	got, ok := eng.SetPRThreadResolved(pr.ID, "PRRT_1", true)
	if !ok {
		t.Fatal("SetPRThreadResolved() ok = false, want true")
	}
	if refreshed {
		t.Error("SetPRThreadResolved() called RefreshPRConversation, want local persist")
	}
	if len(got.Comments) != 1 || !got.Comments[0].Resolved {
		t.Errorf("SetPRThreadResolved() = %+v, want gh-inline-1 resolved", got.Comments)
	}
	var payload map[string]any
	for _, ev := range sink.byName("pr.updated") {
		d, ok := ev.Data.(map[string]any)
		if !ok || d["prId"] != pr.ID {
			continue
		}
		payload = d
	}
	if payload["pendingOnly"] != true {
		t.Errorf("SetPRThreadResolved() pendingOnly = %v, want true", payload["pendingOnly"])
	}
}

func TestRefreshPRConversationKeepsRecentResolve(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "hold", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-30", WorkspaceID: ws.ID, Number: 30, Source: domain.SourceGitHub,
		ViewerLogin: "avery", ViewerCanWrite: true,
		Comments: []domain.PRComment{{
			ID: "gh-inline-1", Path: "auth.go", Line: 12, ThreadID: "PRRT_1", BodyMD: "hmm", Resolved: true,
		}},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	eng.rememberThreadResolved(pr.ID, "PRRT_1", true)
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			State: "OPEN",
			Comments: []domain.PRComment{{
				ID: "gh-inline-1", Path: "auth.go", Line: 12, ThreadID: "PRRT_1", BodyMD: "hmm", Resolved: false,
			}},
		}, nil
	}
	eng.RefreshPRConversation(pr.ID)
	got, _ := st.PR(pr.ID)
	if len(got.Comments) != 1 || !got.Comments[0].Resolved {
		t.Errorf("RefreshPRConversation() comments = %+v, want gh-inline-1 still resolved", got.Comments)
	}
}

func TestCommentsEqualDetectsOutdated(t *testing.T) {
	t.Parallel()
	live := []domain.PRComment{{
		ID: "gh-inline-1", Path: "b.go", Line: 9, StartLine: 7, Side: "LEFT",
		DiffHunk: "@@ -7,3 +7,3 @@", BodyMD: "stale", BodyHTML: "<code>b.go:7-9</code>",
	}}
	stale := append([]domain.PRComment(nil), live...)
	stale[0].Outdated = true
	if !commentsEqual(live, live) {
		t.Error("commentsEqual(live, live) = false, want true")
	}
	if commentsEqual(live, stale) {
		t.Error("commentsEqual(live, stale) = true, want false when Outdated differs")
	}
}

func TestRefreshPRConversationPersistsOutdated(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, &ScriptedRunner{})
	projID := addProject(t, st, initRepo(t))
	ws, err := eng.CreateWorkspace(projID, "outdated", "")
	if err != nil {
		t.Fatalf("CreateWorkspace: %v", err)
	}
	stored := domain.PRComment{
		ID: "gh-inline-1", Path: "b.go", Line: 9, StartLine: 7, Side: "LEFT",
		DiffHunk: "@@ -7,3 +7,3 @@", BodyMD: "stale", BodyHTML: "<code>b.go:7-9</code>",
	}
	pr, err := st.AddPR(domain.PR{
		ID: "gh-31", WorkspaceID: ws.ID, Number: 31, Source: domain.SourceGitHub,
		Comments: []domain.PRComment{stored},
	})
	if err != nil {
		t.Fatalf("AddPR: %v", err)
	}
	prevSnap := getGitHubPRSnapshot
	t.Cleanup(func() { getGitHubPRSnapshot = prevSnap })
	fresh := stored
	fresh.Outdated = true
	getGitHubPRSnapshot = func(context.Context, string, int) (ghPRSnapshot, error) {
		return ghPRSnapshot{
			State:    "OPEN",
			Comments: []domain.PRComment{fresh},
		}, nil
	}
	if !eng.RefreshPRConversation(pr.ID) {
		t.Error("RefreshPRConversation() = false, want true when a stored comment becomes outdated")
	}
	got, _ := st.PR(pr.ID)
	if len(got.Comments) != 1 || !got.Comments[0].Outdated || got.Comments[0].Line != 9 {
		t.Errorf("RefreshPRConversation() comments = %+v, want gh-inline-1 Outdated with line 9", got.Comments)
	}
}
