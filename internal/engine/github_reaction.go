package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"time"

	"recur/internal/domain"
)

// setGitHubReactionFn adds or removes a reaction on a GitHub subject. Tests replace this.
var setGitHubReactionFn = setGitHubReaction

type ghReactionHit struct {
	IDs       []string
	NodeID    string
	Reactions []domain.PRReaction
}

type ghReactionGroupJSON struct {
	Content          string `json:"content"`
	ViewerHasReacted bool   `json:"viewerHasReacted"`
	Reactors         struct {
		TotalCount int `json:"totalCount"`
	} `json:"reactors"`
	Users struct {
		TotalCount int `json:"totalCount"`
	} `json:"users"`
}

func (g ghReactionGroupJSON) count() int {
	if g.Reactors.TotalCount > 0 {
		return g.Reactors.TotalCount
	}
	return g.Users.TotalCount
}

func parseReactionGroups(groups []ghReactionGroupJSON) []domain.PRReaction {
	var out []domain.PRReaction
	for _, g := range groups {
		n, ok := domain.NormalizeReactionContent(g.Content)
		if !ok {
			continue
		}
		count := g.count()
		if count <= 0 {
			continue
		}
		out = append(out, domain.PRReaction{Content: n, Count: count, Mine: g.ViewerHasReacted})
	}
	return domain.SortReactions(out)
}

func fetchGitHubReactions(ctx context.Context, repoPath, ownerRepo string, prNumber int) ([]ghReactionHit, string, error) {
	owner, repo, ok := splitOwnerRepo(ownerRepo)
	if !ok || prNumber <= 0 {
		return nil, "", fmt.Errorf("invalid repo for reactions")
	}
	const q = `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){id reactionGroups{content viewerHasReacted reactors{totalCount}}comments(last:100){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}reviews(last:100){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}reviewThreads(first:100){nodes{comments(first:50){nodes{id databaseId reactionGroups{content viewerHasReacted reactors{totalCount}}}}}}}}}`
	out, err := runGH(ctx, repoPath, "api", "graphql",
		"-f", "query="+q,
		"-F", "o="+owner,
		"-F", "r="+repo,
		"-F", "n="+strconv.Itoa(prNumber))
	if err != nil {
		return nil, "", err
	}
	return parseGitHubReactions(out)
}

type ghReactableNode struct {
	ID             string                `json:"id"`
	DatabaseID     int64                 `json:"databaseId"`
	ReactionGroups []ghReactionGroupJSON `json:"reactionGroups"`
}

func parseGitHubReactions(raw string) ([]ghReactionHit, string, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, "", fmt.Errorf("empty reaction graphql")
	}
	var payload struct {
		Data struct {
			Repository struct {
				PullRequest struct {
					ID             string                `json:"id"`
					ReactionGroups []ghReactionGroupJSON `json:"reactionGroups"`
					Comments       struct {
						Nodes []ghReactableNode `json:"nodes"`
					} `json:"comments"`
					Reviews struct {
						Nodes []ghReactableNode `json:"nodes"`
					} `json:"reviews"`
					ReviewThreads struct {
						Nodes []struct {
							Comments struct {
								Nodes []ghReactableNode `json:"nodes"`
							} `json:"comments"`
						} `json:"nodes"`
					} `json:"reviewThreads"`
				} `json:"pullRequest"`
			} `json:"repository"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(raw), &payload); err != nil {
		return nil, "", fmt.Errorf("parse reaction graphql: %w", err)
	}
	pr := payload.Data.Repository.PullRequest
	prNode := strings.TrimSpace(pr.ID)
	var hits []ghReactionHit
	if prNode != "" {
		hits = append(hits, ghReactionHit{
			IDs:       []string{"gh-description"},
			NodeID:    prNode,
			Reactions: parseReactionGroups(pr.ReactionGroups),
		})
	}
	for _, n := range pr.Comments.Nodes {
		hits = append(hits, reactionHit(n, "gh-comment-"))
	}
	for _, n := range pr.Reviews.Nodes {
		h := reactionHit(n, "gh-review-")
		if id := strings.TrimSpace(n.ID); id != "" {
			h.IDs = append(h.IDs, "gh-review-action-"+id)
		}
		if n.DatabaseID > 0 {
			h.IDs = append(h.IDs, fmt.Sprintf("gh-review-action-%d", n.DatabaseID))
		}
		hits = append(hits, h)
	}
	for _, th := range pr.ReviewThreads.Nodes {
		for _, n := range th.Comments.Nodes {
			hits = append(hits, reactionHit(n, "gh-inline-"))
		}
	}
	return hits, prNode, nil
}

func reactionHit(n ghReactableNode, prefix string) ghReactionHit {
	id := strings.TrimSpace(n.ID)
	h := ghReactionHit{
		NodeID:    id,
		Reactions: parseReactionGroups(n.ReactionGroups),
	}
	if id != "" {
		h.IDs = append(h.IDs, prefix+id)
	}
	if n.DatabaseID > 0 {
		h.IDs = append(h.IDs, fmt.Sprintf("%s%d", prefix, n.DatabaseID))
	}
	return h
}

func applyGitHubReactions(cs []domain.PRComment, hits []ghReactionHit) {
	if len(hits) == 0 {
		return
	}
	byID := map[string]ghReactionHit{}
	for _, h := range hits {
		for _, id := range h.IDs {
			if id != "" {
				byID[id] = h
			}
		}
	}
	for i, c := range cs {
		h, ok := byID[c.ID]
		if !ok {
			continue
		}
		cs[i].Reactions = h.Reactions
		if h.NodeID != "" {
			cs[i].NodeID = h.NodeID
		}
	}
}

func setGitHubReaction(ctx context.Context, repoPath, subjectID, content string, add bool) error {
	subjectID = strings.TrimSpace(subjectID)
	gql, ok := domain.GraphQLReactionContent(content)
	if subjectID == "" || !ok {
		return fmt.Errorf("invalid reaction subject")
	}
	mut := "addReaction"
	if !add {
		mut = "removeReaction"
	}
	query := fmt.Sprintf(
		`mutation($id:ID!,$c:ReactionContent!){%s(input:{subjectId:$id,content:$c}){reaction{content}}}`, mut)
	out, err := runGH(ctx, repoPath, "api", "graphql",
		"-f", "query="+query,
		"-F", "id="+subjectID,
		"-F", "c="+gql)
	if err != nil {
		return err
	}
	var payload struct {
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if json.Unmarshal([]byte(out), &payload) == nil && len(payload.Errors) > 0 {
		msg := strings.TrimSpace(payload.Errors[0].Message)
		if msg == "" {
			msg = "GitHub reaction failed"
		}
		return fmt.Errorf("%s", msg)
	}
	return nil
}

func reactionSubjectID(c domain.PRComment, pr domain.PR) string {
	if id := strings.TrimSpace(c.NodeID); id != "" {
		return id
	}
	if c.ID == "gh-description" || c.Kind == "description" {
		return strings.TrimSpace(pr.NodeID)
	}
	id := strings.TrimSpace(c.ID)
	for _, p := range []string{"gh-comment-", "gh-review-action-", "gh-review-", "gh-inline-"} {
		if !strings.HasPrefix(id, p) {
			continue
		}
		rest := strings.TrimPrefix(id, p)
		if rest == "" {
			return ""
		}
		if _, err := strconv.ParseInt(rest, 10, 64); err == nil {
			return ""
		}
		return rest
	}
	return ""
}

type commentReactionHold struct {
	Reactions []domain.PRReaction
	At        time.Time
}

func cloneReactions(rs []domain.PRReaction) []domain.PRReaction {
	if len(rs) == 0 {
		return nil
	}
	out := make([]domain.PRReaction, len(rs))
	copy(out, rs)
	return out
}

func (e *Engine) rememberCommentReactions(prID, commentID string, rs []domain.PRReaction) {
	if e == nil || prID == "" || commentID == "" {
		return
	}
	e.commentReactionsMu.Lock()
	defer e.commentReactionsMu.Unlock()
	if e.commentReactions == nil {
		e.commentReactions = map[string]commentReactionHold{}
	}
	e.commentReactions[droppedCommentKey(prID, commentID)] = commentReactionHold{
		Reactions: cloneReactions(rs),
		At:        time.Now(),
	}
}

func (e *Engine) forgetCommentReactions(prID, commentID string) {
	if e == nil || e.commentReactions == nil {
		return
	}
	e.commentReactionsMu.Lock()
	defer e.commentReactionsMu.Unlock()
	delete(e.commentReactions, droppedCommentKey(prID, commentID))
}

func (e *Engine) applyRememberedCommentReactions(prID string, cs []domain.PRComment) []domain.PRComment {
	if e == nil || len(cs) == 0 {
		return cs
	}
	e.commentReactionsMu.Lock()
	defer e.commentReactionsMu.Unlock()
	if len(e.commentReactions) == 0 {
		return cs
	}
	now := time.Now()
	holds := map[string][]domain.PRReaction{}
	for key, hold := range e.commentReactions {
		if now.Sub(hold.At) >= droppedCommentTTL {
			delete(e.commentReactions, key)
			continue
		}
		_, id, ok := strings.Cut(key, "\x00")
		if !ok || !strings.HasPrefix(key, prID+"\x00") {
			continue
		}
		holds[id] = hold.Reactions
	}
	if len(holds) == 0 {
		return cs
	}
	out := append([]domain.PRComment(nil), cs...)
	for i, c := range out {
		if rs, ok := holds[c.ID]; ok {
			out[i].Reactions = cloneReactions(rs)
		}
	}
	return out
}
