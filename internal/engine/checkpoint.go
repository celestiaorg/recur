package engine

import (
	"context"
	"fmt"
	"log"
	"path/filepath"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
	"recur/internal/store"
)

// Chat checkpoints: every agent turn snapshots the worktree twice under
// refs/recur/checkpoints/<workspace>/<userMessageID>/{start,end} (see
// internal/git/checkpoint.go for the invisibility guarantees). start→end is
// the turn's own diff; end is what "reset to here" restores.

func checkpointRef(workspaceID, userMsgID, phase string) string {
	return gitpkg.CheckpointRefPrefix + workspaceID + "/" + userMsgID + "/" + phase
}

// takeCheckpoint snapshots ws for one turn phase. Failures are logged, never
// surfaced — a send must not fail because git could not snapshot.
func (e *Engine) takeCheckpoint(ws domain.Workspace, userMsgID, phase string) {
	if ws.WorktreePath == "" || userMsgID == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	if _, err := e.checkpoint(ctx, ws, checkpointRef(ws.ID, userMsgID, phase)); err != nil {
		log.Printf("checkpoint %s %s/%s: %v", phase, ws.ID, userMsgID, err)
	}
}

// checkpoint snapshots ws into ref, staging through the private index kept in
// the workspace data dir (never the worktree's own index).
func (e *Engine) checkpoint(ctx context.Context, ws domain.Workspace, ref string) (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	dir, err := store.WorkspaceDir(root, ws.ID)
	if err != nil {
		return "", err
	}
	return gitpkg.Checkpoint(ctx, ws.WorktreePath, filepath.Join(dir, "checkpoint-index"), ref)
}

// turnUserMessageID maps a turn footer (the message stamped with DurationMS)
// to the human message that opened that turn — the checkpoint key.
func turnUserMessageID(msgs []domain.Message, footerID string) string {
	user := ""
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			user = m.ID
		}
		if m.ID == footerID {
			return user
		}
	}
	return ""
}

// lastChatPromptID is the id of the tab's most recent human message — the turn
// a retry or switch-mode remount continues.
func lastChatPromptID(st *store.Store, workspaceID, code string) string {
	sess, ok := st.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return ""
	}
	msgs, err := st.SessionTranscript(sess.ID)
	if err != nil {
		return ""
	}
	id := ""
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			id = m.ID
		}
	}
	return id
}

// lastOpenChatPromptID is the in-flight user message a restart remount may
// continue: it has a start checkpoint and no DurationMS footer yet. An
// interim end (graceful quit snapshots after cancel) is replaced when the
// continuation finishes. Empty means AutoResume must open a distinct turn
// so a completed prompt's end ref is not overwritten.
func (e *Engine) lastOpenChatPromptID(ws domain.Workspace, code string) string {
	if e == nil || e.Store == nil || ws.WorktreePath == "" {
		return ""
	}
	sess, ok := e.Store.SessionForLoopAgent(ws.ID, code)
	if !ok {
		return ""
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		return ""
	}
	userID := ""
	closed := false
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			userID = m.ID
			closed = false
		} else if userID != "" && m.DurationMS > 0 {
			closed = true
		}
	}
	if userID == "" || closed {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	start := checkpointRef(ws.ID, userID, "start")
	if gitpkg.ListCheckpoints(ctx, ws.WorktreePath, start)[start] == "" {
		return ""
	}
	return userID
}

// markCheckpoints flags turn footers whose start→end snapshot actually
// changed files, so the UI only offers "diff this turn" where it would show
// something.
func (e *Engine) markCheckpoints(ws domain.Workspace, msgs []domain.Message, items []domain.ChatItem) {
	if ws.WorktreePath == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	trees := gitpkg.ListCheckpointTrees(ctx, ws.WorktreePath, gitpkg.CheckpointRefPrefix+ws.ID+"/")
	if len(trees) == 0 {
		return
	}
	// Associate every footer with its prompt in one pass through the history.
	users := make(map[string]string, len(msgs))
	user := ""
	for _, m := range msgs {
		if m.Role == "user" && userMessageIsChatPrompt(m) {
			user = m.ID
		}
		users[m.ID] = user
	}
	for i := range items {
		if items[i].Kind != "turn" {
			continue
		}
		user := users[items[i].MessageID]
		if user == "" {
			continue
		}
		start, end := trees[checkpointRef(ws.ID, user, "start")], trees[checkpointRef(ws.ID, user, "end")]
		items[i].Checkpoint = start != "" && end != "" && start != end
	}
}

// chatTurn resolves a footer id on a tab to (workspace, session, user message id).
func (e *Engine) chatTurn(workspaceID, code, footerID string) (domain.Workspace, domain.Session, string, error) {
	code = normalizePairChatCode(code)
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return domain.Workspace{}, domain.Session{}, "", fmt.Errorf("workspace not found")
	}
	sess, ok := e.Store.SessionForLoopAgent(workspaceID, code)
	if !ok {
		return domain.Workspace{}, domain.Session{}, "", fmt.Errorf("chat has no session")
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		return domain.Workspace{}, domain.Session{}, "", err
	}
	userID := turnUserMessageID(msgs, footerID)
	if userID == "" {
		return domain.Workspace{}, domain.Session{}, "", fmt.Errorf("turn not found")
	}
	return ws, sess, userID, nil
}

// WorkspaceChatTurnDiff is what the agent changed during the turn that ends
// at footer messageID: its start checkpoint against its end checkpoint (or
// the worktree as it is now when the end was never captured). Shaped as a
// Commit so the Observatory's review surface shows it as a commit scope.
func (e *Engine) WorkspaceChatTurnDiff(workspaceID, code, messageID string) (domain.Commit, error) {
	ws, _, userID, err := e.chatTurn(workspaceID, code, messageID)
	if err != nil {
		return domain.Commit{}, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	refs := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, checkpointRef(ws.ID, userID, ""))
	start, end := checkpointRef(ws.ID, userID, "start"), checkpointRef(ws.ID, userID, "end")
	if refs[start] == "" {
		return domain.Commit{}, fmt.Errorf("no checkpoint for this turn")
	}
	if refs[end] == "" {
		e.takeCheckpoint(ws, userID, "end")
		refs = gitpkg.ListCheckpoints(ctx, ws.WorktreePath, end)
	}
	files, err := gitpkg.DiffCheckpoints(ctx, ws.WorktreePath, start, end)
	if err != nil {
		return domain.Commit{}, err
	}
	// No Message on purpose: the review surface skips its summary card for a
	// message-less scope — only the file diffs matter here.
	c := domain.Commit{Hash: refs[end], SHA: refs[end], Files: files}
	if len(c.Hash) > 7 {
		c.Hash = c.Hash[:7]
	}
	for _, f := range files {
		c.Diff.Add += f.Diff.Add
		c.Diff.Del += f.Diff.Del
	}
	return c, nil
}

// ResetWorkspaceChat rewinds a chat tab to the end of the turn closed by
// footer messageID: every later message is dropped and, when a later turn
// changed files, the worktree is restored to that turn's end checkpoint. The
// provider session is discarded; the next send re-injects the truncated
// transcript (same path as fork), so the agent continues from exactly what the
// user still sees.
func (e *Engine) ResetWorkspaceChat(workspaceID, code, messageID string) error {
	code = normalizePairChatCode(code)
	if e.chatIsRunning(workspaceID, code) {
		return fmt.Errorf("stop the running turn first")
	}
	ws, sess, userID, err := e.chatTurn(workspaceID, code, messageID)
	if err != nil {
		return err
	}
	// Maestro-routed sends never replay forkInject, so a reset would wipe the
	// agent's memory while the UI still shows the earlier turns.
	if code == maestroAgentCode || e.chatLoopStarted(ws.ID, code) || (ws.HasRun() && loopOwnerCode(ws) == code) {
		return fmt.Errorf("reset is not available on a running loop's chat")
	}
	msgs, err := e.Store.SessionTranscript(sess.ID)
	if err != nil {
		return err
	}
	// Leave the worktree alone when only conversation follows, so manual
	// edits made since are not discarded for nothing.
	if e.editedAfter(ws, msgs, messageID) {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()
		end := checkpointRef(ws.ID, userID, "end")
		if gitpkg.ListCheckpoints(ctx, ws.WorktreePath, end)[end] == "" {
			return fmt.Errorf("no checkpoint for this turn")
		}
		if err := gitpkg.RestoreCheckpoint(ctx, ws.WorktreePath, end); err != nil {
			return err
		}
	}
	if _, err := e.Store.TruncateSessionAfter(sess.ID, messageID); err != nil {
		return err
	}
	e.clearAgentSession(workspaceID, code)
	return e.Store.PatchSessionConfigMerge(sess.ID, `{"forkInject":true}`)
}

// editedAfter reports whether any turn after footer footerID changed files,
// by the same rule that flags footers in the UI.
func (e *Engine) editedAfter(ws domain.Workspace, msgs []domain.Message, footerID string) bool {
	var later []domain.ChatItem
	past := false
	for _, m := range msgs {
		if past && m.DurationMS > 0 {
			later = append(later, domain.ChatItem{Kind: "turn", MessageID: m.ID})
		}
		past = past || m.ID == footerID
	}
	e.markCheckpoints(ws, msgs, later)
	for _, it := range later {
		if it.Checkpoint {
			return true
		}
	}
	return false
}

// ReviewCheckpoint snapshots the worktree for a /review or /re-review pass on
// a chat tab and returns (since, now) oids when the pass should embed only
// what changed since the tab's previous review pass: /re-review on a tab that
// already reviewed once and still has a live provider session to resume. Any
// other case returns "", "" — the caller embeds the full diff in a fresh session.
func (e *Engine) ReviewCheckpoint(workspaceID, code, slash string) (since, now string) {
	if slash != "review" && slash != "re-review" {
		return "", ""
	}
	code = normalizePairChatCode(code)
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok || ws.WorktreePath == "" {
		return "", ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	ref := gitpkg.CheckpointRefPrefix + ws.ID + "/review/" + code
	prev := gitpkg.ListCheckpoints(ctx, ws.WorktreePath, ref)[ref]
	oid, err := e.checkpoint(ctx, ws, ref)
	if err != nil {
		log.Printf("review checkpoint %s/%s: %v", ws.ID, code, err)
		return "", ""
	}
	a, _ := e.Store.Agent(workspaceID, code)
	if slash != "re-review" || prev == "" || a.SessionID == "" {
		return "", ""
	}
	return prev, oid
}

// chatIsRunning reports whether the tab's agent has a turn in flight.
func (e *Engine) chatIsRunning(workspaceID, code string) bool {
	a, ok := e.Store.Agent(workspaceID, code)
	return ok && a.Status == "working"
}

// pruneCheckpoints drops every checkpoint ref a deleted workspace left in the
// project repo (worktree refs live in the shared object store).
func (e *Engine) pruneCheckpoints(repoPath, workspaceID string) {
	if repoPath == "" || workspaceID == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	_ = gitpkg.DeleteCheckpoints(ctx, repoPath, gitpkg.CheckpointRefPrefix+workspaceID+"/")
}
