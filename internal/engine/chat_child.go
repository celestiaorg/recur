package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"html"
	"sort"
	"strings"
	"time"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// Children of Agent chats (create_workspace without standalone): the Recur
// subagent. A chat child is a workspace with ParentID set and no
// ParentStepIDs; loop fan-out children carry step ids and are merged by their
// parent's board instead. When a chat child settles the parent chat gets ONE
// turn carrying its report, then decides: merge_child, archive_child, or keep
// reading it. FanOut.Outcome on the child row is the lifecycle marker the
// card is rebuilt from: "" running, settled, merged, closed. report-pending
// is an internal delivery marker, not shown on the card.

const (
	childOutcomeSettled       = "settled"
	childOutcomeMerged        = "merged"
	childOutcomeClosed        = "closed"
	childOutcomeReportPending = "report-pending"
	childReplyMax             = 4000
	childNoticeReplyMax       = 1200
	childDiffMax              = 12000
	parentSteerPrefix         = "Parent workspace sent this while you were working. Treat it as additional instruction and continue:\n\n"
)

func isChatChild(w domain.Workspace) bool {
	return strings.TrimSpace(w.ParentID) != "" && len(w.ParentStepIDs) == 0
}

// chatChild loads a child of parentID, archived rows included.
func (e *Engine) chatChild(parentID, childID string) (domain.Workspace, bool) {
	child, ok := e.Store.Workspace(childID)
	if !ok || child.ParentID != parentID || !isChatChild(child) {
		return domain.Workspace{}, false
	}
	return child, true
}

// childChat is the parent tab a chat child reports to.
func childChat(w domain.Workspace) string {
	if w.FanOut != nil && strings.TrimSpace(w.FanOut.Chat) != "" {
		return w.FanOut.Chat
	}
	return pairAgentCode
}

// childSettledForChat: a chat child's row never leaves its creation state
// (queued/paused) on plain turns, so settled means no loop goroutine, no
// live chat turn or agent, and the launch's first turn finished. A child
// that engaged a loop defers to the loop rule.
func (e *Engine) childSettledForChat(childID string) bool {
	ws, ok := e.Store.Workspace(childID)
	if !ok {
		return true
	}
	if e.runningLoopByID(childID) != nil || e.workspaceHasLiveChat(childID) || e.chatResuming(childID) || ws.ScheduledPromptPending {
		return false
	}
	if e.askUsers != nil && e.askUsers.workspaceBusy(childID) {
		return false
	}
	for _, a := range e.Store.Agents(childID) {
		if isLiveAgentStatus(a.Status) {
			return false
		}
	}
	if strings.TrimSpace(ws.LoopChatCode) != "" {
		return e.childSettled(childID)
	}
	switch ws.State {
	case domain.StatePlanning, domain.StateCoding, domain.StateReviewing, domain.StateVerifying:
		return false
	}
	return true
}

// watchChatChild reports the child to its parent chat once it settles.
func (e *Engine) watchChatChild(parentID, childID string) {
	if _, dup := e.chatChildWatchers.LoadOrStore(childID, struct{}{}); dup {
		return
	}
	defer e.chatChildWatchers.Delete(childID)
	for {
		if e.isShuttingDown() {
			return
		}
		child, ok := e.chatChild(parentID, childID)
		if !ok || child.Archived {
			return
		}
		if e.chatChildReadyToReport(child) && e.reportChild(parentID, child) {
			// The parent's report turn may have messaged the child into a
			// new turn (reopenChatChild); keep watching for its next report.
			if c, ok := e.chatChild(parentID, childID); !ok || chatChildOutcomeDone(c) {
				return
			}
		}
		time.Sleep(e.fanOutPoll)
	}
}

func childCreateFailed(w domain.Workspace) bool {
	return w.State == domain.StateFailed || w.Phase == "failed" || strings.TrimSpace(w.LastError) != ""
}

func chatChildOutcomeDone(w domain.Workspace) bool {
	if w.FanOut == nil || w.FanOut.Outcome == "" || w.FanOut.Outcome == childOutcomeReportPending {
		return false
	}
	return true
}

func (e *Engine) chatChildReadyToReport(child domain.Workspace) bool {
	if chatChildOutcomeDone(child) || !e.childSettledForChat(child.ID) {
		return false
	}
	return child.WorktreePath != "" || childCreateFailed(child)
}

// rearmChatChildren restarts the watchers a restart dropped: unreported,
// unarchived children whose parent is still around. A report that was
// pending when the app died but whose notice is already on the parent tab is
// marked settled, not resent — the parent got it (and AutoResume replays its
// interrupted turn).
func (e *Engine) rearmChatChildren() {
	for _, w := range e.Store.Workspaces() {
		if w.Archived || !isChatChild(w) || chatChildOutcomeDone(w) {
			continue
		}
		if w.FanOut != nil && w.FanOut.Outcome == childOutcomeReportPending && e.noticeDelivered(w.ParentID, childChat(w), w) {
			e.markChildReported(w.ParentID, w.ID)
			continue
		}
		if p, ok := e.Store.Workspace(w.ParentID); ok && !p.Archived {
			go e.watchChatChild(w.ParentID, w.ID)
		}
	}
}

// reopenChatChild re-watches a reported child that starts a new turn (a
// human retry after a usage limit, a follow-up message), so its parent gets
// another report when it settles again. Call once the turn is live.
func (e *Engine) reopenChatChild(childID string) {
	reopened := false
	updated, _ := e.Store.UpdateWorkspace(childID, func(w *domain.Workspace) {
		if !w.Archived && isChatChild(*w) && w.FanOut != nil && (w.FanOut.Outcome == childOutcomeSettled || w.FanOut.Outcome == childOutcomeReportPending || strings.HasPrefix(w.FanOut.Outcome, "conflict: ")) {
			w.FanOut.Outcome, w.FanOut.Note = "", ""
			reopened = true
		}
	})
	if !reopened {
		return
	}
	if p, ok := e.Store.Workspace(updated.ParentID); !ok || p.Archived {
		return
	}
	e.emitChildCard(updated.ParentID, updated)
	go e.watchChatChild(updated.ParentID, childID)
}

// setChatChildOutcome stamps the lifecycle marker and refreshes the card.
func (e *Engine) setChatChildOutcome(parentID, childID, outcome, note string) {
	updated, ok := e.Store.UpdateWorkspace(childID, func(w *domain.Workspace) {
		if w.FanOut == nil {
			w.FanOut = &domain.ChildFanOut{}
		}
		w.FanOut.Outcome, w.FanOut.Note = outcome, note
	})
	if ok && outcome != childOutcomeReportPending {
		e.emitChildCard(parentID, updated)
	}
}

// chatChildWave returns the subagent wave for a child the parent tab is
// creating now: siblings created by the same live parent turn share a wave
// (and a card); a new turn opens the next wave. Numbered from the parent's
// FanOutWaves counter, like loop waves.
func (e *Engine) chatChildWave(parentID, code string) int {
	turn := e.liveChatTurn(parentID, code)
	if turn != nil {
		if v, ok := e.childWaves.Load(turn); ok {
			return v.(int)
		}
	}
	wave := 0
	e.Store.UpdateWorkspace(parentID, func(w *domain.Workspace) {
		w.FanOutWaves++
		wave = w.FanOutWaves
	})
	if turn != nil {
		e.childWaves.Store(turn, wave)
	}
	return wave
}

// emitChildCard (re)publishes the card for the child's wave on the parent
// feed under a stable id: one row per sibling created by the same parent
// turn, no step chips. Archived siblings stay on the card.
func (e *Engine) emitChildCard(parentID string, child domain.Workspace) {
	wave := 0
	if child.FanOut != nil {
		wave = child.FanOut.Wave
	}
	chat := childChat(child)
	var rows []fanOutChild
	done := true
	for _, c := range e.Store.Workspaces() {
		if c.ParentID != parentID || !isChatChild(c) || c.FanOut == nil || c.FanOut.Wave != wave || childChat(c) != chat {
			continue
		}
		row := fanOutChild{ID: c.ID, Title: c.Goal, Steps: []string{}, Mode: c.FanOut.Mode, State: c.FanOut.Outcome, Note: c.FanOut.Note}
		if row.State != childOutcomeMerged && row.State != childOutcomeClosed {
			done = false
		}
		rows = append(rows, row)
	}
	if len(rows) == 0 {
		return
	}
	title := "Subagent"
	if len(rows) > 1 {
		title = "Subagents"
	}
	data, err := json.Marshal(fanOutWave{Wave: wave, Title: title, Children: rows, Done: done})
	if err != nil {
		return
	}
	var b strings.Builder
	fmt.Fprintf(&b, "%s: %d child workspace(s) — ", title, len(rows))
	for i, r := range rows {
		if i > 0 {
			b.WriteString(" · ")
		}
		state := r.State
		if state == "" {
			state = "running"
		}
		fmt.Fprintf(&b, "%s (%s)", html.EscapeString(r.Title), html.EscapeString(state))
	}
	e.emitWorkspaceCard(parentID, domain.FeedItem{
		ID: fmt.Sprintf("fanout-%s-c%d-%s", parentID, wave, chat), AgentCode: "MA", Name: "Recur",
		Text: b.String(), ChatCode: chat, Card: domain.FeedCardFanOut, Data: string(data),
	})
}

// reportChild sends ONE turn to the parent tab that created the child. The
// input is agent-to-agent, so its bubble is hidden; the parent's reply and the
// child card are what the human sees. The tab slot queues it behind a human
// send. Delivery is the notice persisted as a user message on the parent tab
// (noticeDelivered) — not the reply text, which is empty for a parked
// ask_user, "Stopped by user" when the human clicks Stop, or whatever the
// model said. A send the engine refused persists nothing, so it is retried.
func (e *Engine) reportChild(parentID string, child domain.Workspace) bool {
	e.setChatChildOutcome(parentID, child.ID, childOutcomeReportPending, "")
	chat := childChat(child)
	before := e.noticeCount(parentID, chat, child)
	model, flags, mode := e.WorkspaceChatSelection(parentID, chat)
	reply := e.SendWorkspaceMessage(parentID, chat, e.childNotice(parentID, child), model, flags, nil, mode,
		childNoticeDisplay(child))
	if e.noticeCount(parentID, chat, child) > before {
		e.markChildReported(parentID, child.ID)
		return true
	}
	if reply == chatStoppedReply {
		// The human clicked Stop while the notice was queued behind a live
		// turn (or waiting for a slot), so it was never persisted. Honor the
		// Stop: no automatic retry. The child's row on the card carries the
		// explanation — plain feed lines do not render on Agent tabs.
		e.markChildReported(parentID, child.ID, fmt.Sprintf("report cancelled by Stop — ask this chat to read_child %s", child.ID))
		return true
	}
	return false
}

// noticeDelivered reports whether the child's notice reached the parent tab:
// its persisted user message carries the notice display text. Restart-safe.
func (e *Engine) noticeDelivered(parentID, chat string, child domain.Workspace) bool {
	return e.noticeCount(parentID, chat, child) > 0
}

// noticeCount counts the child's notices on the parent tab; a reopened child
// reports again under the same display text.
func (e *Engine) noticeCount(parentID, chat string, child domain.Workspace) int {
	want := strings.TrimSpace(childNoticeDisplay(child))
	n := 0
	for _, it := range e.WorkspaceChatItems(parentID, chat) {
		if it.Role == "user" && it.Kind == "text" && strings.TrimSpace(it.Text) == want {
			n++
		}
	}
	return n
}

// markChildReported moves report-pending to settled. The parent's report turn
// runs synchronously and is told to merge_child / archive_child, so by the
// time it returns the outcome may already be merged, closed, or conflict —
// those stand; only a still-pending marker is advanced.
func (e *Engine) markChildReported(parentID, childID string, note ...string) {
	updated, ok := e.Store.UpdateWorkspace(childID, func(w *domain.Workspace) {
		if w.FanOut != nil && w.FanOut.Outcome == childOutcomeReportPending {
			w.FanOut.Outcome = childOutcomeSettled
			if len(note) > 0 {
				w.FanOut.Note = note[0]
			}
		}
	})
	if ok && updated.FanOut != nil && updated.FanOut.Outcome == childOutcomeSettled {
		e.emitChildCard(parentID, updated)
	}
}

// childNoticeDisplay is the notice turn's display text: the hidden marker
// (sendWorkspaceTurn: never steer, run as own turn) followed by the label
// the transcript renders as a "Subagent" notice row instead of a user bubble.
func childNoticeDisplay(child domain.Workspace) string {
	return displayHiddenSentinel + fmt.Sprintf("%s finished (%s)", child.Goal, child.ID)
}

// childNotice is the short re-engage turn: state, the reply (capped), how
// many files it committed, and the verbs. read_child has the rest.
func (e *Engine) childNotice(parentID string, child domain.Workspace) string {
	var b strings.Builder
	fmt.Fprintf(&b, "Subagent %s (%q) has %s.", child.ID, child.Goal, describeChildState(child))
	if err := strings.TrimSpace(child.LastError); err != "" {
		fmt.Fprintf(&b, " Create failed: %s.", err)
	}
	if paths := e.childChangedPaths(parentID, child); len(paths) > 0 {
		fmt.Fprintf(&b, " It committed %d file(s) on branch %s.", len(paths), child.Branch)
	} else {
		b.WriteString(" It committed no files.")
	}
	if n := len(e.childDirtyPaths(child)); n > 0 {
		fmt.Fprintf(&b, " It has %d uncommitted file(s) that merge_child will commit.", n)
	}
	if reply := strings.TrimSpace(e.Store.LastAssistantText(child.ID)); reply != "" {
		fmt.Fprintf(&b, "\n\nIts reply:\n%s", truncate(reply, childNoticeReplyMax))
	}
	fmt.Fprintf(&b, "\n\nread_child %s for the full report and diff; merge_child %s to land its commits here; archive_child %s to close it without merging. "+
		"Each subagent reports in its own turn; do not assume another child is still running — its report arrives when it finishes.",
		child.ID, child.ID, child.ID)
	return b.String()
}

// childChangedPaths lists the child's committed changes against the parent
// branch; empty for archived children (the worktree is gone).
func (e *Engine) childChangedPaths(parentID string, child domain.Workspace) []string {
	paths, _ := e.childChangedDiff(parentID, child)
	return paths
}

func (e *Engine) childChangedDiff(parentID string, child domain.Workspace) (paths []string, body string) {
	parent, ok := e.Store.Workspace(parentID)
	if !ok || child.WorktreePath == "" || child.Archived {
		return nil, ""
	}
	proj, ok := e.Store.Project(parent.ProjectID)
	if !ok {
		return nil, ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	return e.gitManager(proj.RepoPath).ChangedDiff(ctx, child.WorktreePath, parent.Branch)
}

func (e *Engine) childDirtyPaths(child domain.Workspace) []string {
	if child.WorktreePath == "" || child.Archived {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	st := gitpkg.StatusMap(ctx, child.WorktreePath)
	paths := make([]string, 0, len(st))
	for p := range st {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	return paths
}

// childUncommittedDiff is the worktree's uncommitted changes: tracked
// staged/unstaged edits (git diff HEAD) plus each untracked file diffed
// against /dev/null, so new files show their contents without touching the
// index (merge_child later stages them with add -A).
func childUncommittedDiff(worktree string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	var b strings.Builder
	if out, err := gitpkg.Command(ctx, "-C", worktree, "diff", "HEAD").Output(); err == nil {
		b.Write(out)
	}
	var untracked []string
	for p, st := range gitpkg.StatusMap(ctx, worktree) {
		if st == "u" {
			untracked = append(untracked, p)
		}
	}
	sort.Strings(untracked)
	for _, p := range untracked {
		// --no-index exits 1 when the files differ; the output is still the diff.
		out, _ := gitpkg.Command(ctx, "-C", worktree, "diff", "--no-index", "--", "/dev/null", p).Output()
		b.Write(out)
	}
	return strings.TrimSpace(b.String())
}

// childReport is the read_child body: state, final reply, handoff, changed
// files against the parent branch (with the diff body when withDiff), and
// artifacts, followed by the verbs that act on the child.
func (e *Engine) childReport(parentID string, child domain.Workspace, withDiff bool) string {
	var b strings.Builder
	running := !child.Archived && !e.childSettledForChat(child.ID)
	if running {
		fmt.Fprintf(&b, "Subagent %s (%q, branch %s) is STILL RUNNING. What follows is a partial snapshot; its commits and reply are not final. "+
			"You will get a turn here with its report when it finishes — do not wait, poll, merge, or archive it now.\n\n", child.ID, child.Goal, child.Branch)
	} else {
		fmt.Fprintf(&b, "Subagent %s (%q, branch %s) has %s", child.ID, child.Goal, child.Branch, describeChildState(child))
		if child.Archived {
			b.WriteString("; it is archived")
		}
		b.WriteString(".\n\n")
	}
	if err := strings.TrimSpace(child.LastError); err != "" {
		fmt.Fprintf(&b, "Error: %s\n\n", err)
	}
	if reply := strings.TrimSpace(e.Store.LastAssistantText(child.ID)); reply != "" {
		fmt.Fprintf(&b, "Final reply:\n%s\n\n", truncate(reply, childReplyMax))
	}
	if h := strings.TrimSpace(child.Handoff); h != "" {
		fmt.Fprintf(&b, "Handoff notes: %s\n\n", h)
	}
	if child.Plan != nil {
		if t := strings.TrimSpace(child.Plan.Text()); t != "" {
			fmt.Fprintf(&b, "Plan:\n%s\n\n", truncate(t, childReplyMax))
		}
	}
	if findings := e.Store.Findings(child.ID); len(findings) > 0 {
		b.WriteString("Findings:\n")
		for _, f := range findings {
			loc := domain.FindingLoc(f.File, f.Line, f.EndLine)
			if loc != "" {
				loc = " " + loc
			}
			fmt.Fprintf(&b, "- [%s] %s%s: %s\n", f.Status, f.Severity, loc, f.Message)
		}
		b.WriteString("\n")
	}
	if parent, ok := e.Store.Workspace(parentID); ok && child.WorktreePath != "" && !child.Archived {
		paths, body := e.childChangedDiff(parentID, child)
		if len(paths) == 0 {
			fmt.Fprintf(&b, "Committed changes vs %s: none.\n\n", parent.Branch)
		} else {
			fmt.Fprintf(&b, "Committed changes vs %s (%d file(s)): %s\n", parent.Branch, len(paths), strings.Join(paths, ", "))
			if withDiff && strings.TrimSpace(body) != "" {
				fmt.Fprintf(&b, "```diff\n%s\n```\n", truncate(body, childDiffMax))
			}
			b.WriteString("\n")
		}
		if dirty := e.childDirtyPaths(child); len(dirty) > 0 {
			fmt.Fprintf(&b, "Uncommitted (staged/unstaged/untracked, %d file(s)): %s\n", len(dirty), strings.Join(dirty, ", "))
			if withDiff {
				if out := childUncommittedDiff(child.WorktreePath); out != "" {
					fmt.Fprintf(&b, "```diff\n%s\n```\n", truncate(out, childDiffMax))
				}
			}
			b.WriteString("\n")
		}
	}
	if arts, err := e.ListArtifacts(child.ID); err == nil && len(arts) > 0 {
		b.WriteString("Artifacts:\n")
		for _, a := range arts {
			fmt.Fprintf(&b, "- %s — %s (%s)\n", a.ID, a.Title, a.Kind)
		}
		b.WriteString("\n")
	}
	if !child.Archived && !running {
		fmt.Fprintf(&b, "You can: read_child %s (full diff), read_artifact with workspace_id=%s, merge_child %s to land its commits in this worktree, or archive_child %s to close it without merging.",
			child.ID, child.ID, child.ID, child.ID)
	}
	return b.String()
}

func describeChildState(w domain.Workspace) string {
	switch {
	case w.State == domain.StateFailed || w.State == domain.StateStopped:
		return string(w.State)
	case w.FanOut != nil && w.FanOut.Outcome == childOutcomeMerged:
		return "been merged"
	case w.FanOut != nil && w.FanOut.Outcome == childOutcomeClosed:
		return "been closed"
	default:
		return "finished"
	}
}

// ReadChildFromChat is the read_child tool.
func (e *Engine) ReadChildFromChat(parentID, childID string) string {
	child, ok := e.chatChild(parentID, childID)
	if !ok {
		return fmt.Sprintf("read_child: %s is not a child of this workspace", childID)
	}
	return e.childReport(parentID, child, true)
}

// MessageChildFromChat injects a mid-turn instruction into a running child
// workspace (chat child or loop fan-out), or starts a new turn on a settled
// chat child so it reports again when that turn settles.
func (e *Engine) MessageChildFromChat(parentID, childID, message string) string {
	message = strings.TrimSpace(message)
	if message == "" {
		return "message_child: message is required"
	}
	child, ok := e.Store.Workspace(childID)
	if !ok || parentID == "" || child.ParentID == "" || child.ParentID != parentID || child.Archived {
		return fmt.Sprintf("message_child: %s is not a child of this workspace", childID)
	}
	code := pairAgentCode
	prefix := childID + "\x00"
	e.chats.Range(func(k, _ any) bool {
		key, ok := k.(string)
		if !ok || !strings.HasPrefix(key, prefix) {
			return true
		}
		tab := key[len(prefix):]
		if !isPairChatCode(tab) {
			return true
		}
		if tab == pairAgentCode {
			code = tab
			return false
		}
		if code == pairAgentCode {
			code = tab
		}
		return true
	})
	display := "From parent " + parentID + ": " + message
	if e.tryInjectChat(childID, code, encodeInjectPlain(parentSteerPrefix+message, display)) {
		return "Steered the child mid-turn; the child accepted the message."
	}
	if lp := e.runningLoopByID(childID); lp != nil {
		lp.queueSteer(message)
		return "Steered the child's running loop; this call does not wait."
	}
	if !isChatChild(child) || child.WorktreePath == "" || !e.childSettledForChat(childID) {
		return fmt.Sprintf("message_child: %s is still starting or already finished; wait, or read_child / merge_child / archive_child", childID)
	}
	model, flags, mode := e.WorkspaceChatSelection(childID, code)
	go e.SendWorkspaceMessage(childID, code, message, model, flags, nil, mode, display)
	return "The child had finished, so this started a new turn on it; you get a new turn with its report when it finishes again."
}

// ReadChildArtifactFromChat scopes read_artifact's workspace_id to own children.
func (e *Engine) ReadChildArtifactFromChat(parentID, childID, artifactID string) (string, error) {
	if _, ok := e.chatChild(parentID, childID); !ok {
		return "", fmt.Errorf("read_artifact: %s is not a child of this workspace", childID)
	}
	return e.ReadArtifact(childID, artifactID)
}

// ArchiveChildFromChat is the archive_child tool: close a settled child
// without merging. Its branch is left in git.
func (e *Engine) ArchiveChildFromChat(parentID, childID string) string {
	child, ok := e.chatChild(parentID, childID)
	if !ok || child.Archived {
		return fmt.Sprintf("archive_child: %s is not a live child of this workspace", childID)
	}
	if !e.childSettledForChat(childID) {
		return fmt.Sprintf("archive_child: %s is still running; stop it or wait for it to settle", childID)
	}
	dropped := 0
	if parent, ok := e.Store.Workspace(parentID); ok && child.WorktreePath != "" {
		if proj, ok := e.Store.Project(parent.ProjectID); ok {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			dropped, _ = e.gitManager(proj.RepoPath).CommitsAhead(ctx, child.WorktreePath, parent.Branch)
			cancel()
		}
	}
	e.setChatChildOutcome(parentID, childID, childOutcomeClosed, "")
	updated, ok := e.Store.UpdateWorkspace(childID, func(w *domain.Workspace) {
		w.State, w.StatusLabel, w.Phase = domain.StateStopped, statusLabel(domain.StateStopped), "closed by parent"
		w.Group, w.Checkpoint = domain.GroupReady, nil
	})
	if ok {
		e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	}
	if _, err := e.RemoveWorkspace(childID); err != nil {
		return fmt.Sprintf("archive_child: %s", err)
	}
	if dropped > 0 {
		return fmt.Sprintf("Archived subagent %s (%q) without merging: its worktree and branch %s are deleted, discarding %d unmerged commit(s).", child.ID, child.Goal, child.Branch, dropped)
	}
	return fmt.Sprintf("Archived subagent %s (%q): its worktree and branch %s are deleted; it had no unmerged commits.", child.ID, child.Goal, child.Branch)
}

// MergeChildFromChat is the merge_child tool: merge the child's branch
// --no-ff into the parent worktree now and archive it. The parent is
// mid-turn, so it commits first and resolves conflicts itself.
func (e *Engine) MergeChildFromChat(parentID, childID string) string {
	child, ok := e.chatChild(parentID, childID)
	if !ok || child.Archived {
		return fmt.Sprintf("merge_child: %s is not a live child of this workspace", childID)
	}
	if !e.childSettledForChat(childID) {
		return fmt.Sprintf("merge_child: %s is still running; try again when it settles", childID)
	}
	unlock := e.lockMergeChild(parentID)
	defer unlock()
	child, ok = e.chatChild(parentID, childID)
	if !ok || child.Archived {
		return fmt.Sprintf("merge_child: %s is not a live child of this workspace", childID)
	}
	if !e.childSettledForChat(childID) {
		return fmt.Sprintf("merge_child: %s is still running; try again when it settles", childID)
	}
	if child.WorktreePath == "" {
		return fmt.Sprintf("merge_child: %s has no worktree (it never started); archive_child %s instead", childID, childID)
	}
	parent, ok := e.Store.Workspace(parentID)
	if !ok {
		return "merge_child: parent workspace not found"
	}
	proj, ok := e.Store.Project(parent.ProjectID)
	if !ok {
		return "merge_child: project not found"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	git := e.gitManager(proj.RepoPath)
	label := fmt.Sprintf("subagent %s (%q, branch %s)", child.ID, child.Goal, child.Branch)

	if git.MergeHead(ctx, parent.WorktreePath) {
		return fmt.Sprintf("merge_child: a merge is in progress in this worktree. Resolve it and git commit (or git merge --abort), then call merge_child %s again.", child.ID)
	}
	if n := len(gitpkg.StatusMap(ctx, parent.WorktreePath)); n > 0 {
		return fmt.Sprintf("merge_child: this worktree has %d uncommitted change(s). Commit them first, then call merge_child %s again.", n, child.ID)
	}
	name, email, err := gitpkg.Identity(ctx, parent.WorktreePath)
	if err != nil {
		return "merge_child: " + err.Error()
	}
	if _, err := git.Commit(ctx, child.WorktreePath, "WIP: uncommitted child work", name, email); err != nil {
		return fmt.Sprintf("merge_child: saving the child's uncommitted work failed: %s", err)
	}
	conflicts, err := git.MergeInto(ctx, parent.WorktreePath, child.Branch, "Merge subagent: "+truncate(child.Goal, 60), name, email)
	if err != nil {
		return fmt.Sprintf("merge_child: merging %s failed: %s. Its branch is kept.", label, err)
	}
	if len(conflicts) > 0 {
		e.setChatChildOutcome(parentID, childID, "conflict: "+strings.Join(conflicts, ", "), "")
		return fmt.Sprintf("Merged %s with conflicts in: %s. Resolve the markers, stage the files, run `git commit --no-edit`, then call merge_child %s again to archive it.",
			label, strings.Join(conflicts, ", "), child.ID)
	}
	e.setChatChildOutcome(parentID, childID, childOutcomeMerged, "")
	e.finishChild(child.ID)
	return fmt.Sprintf("Merged %s into this worktree and archived it.", label)
}
