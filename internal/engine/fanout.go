package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"html"
	"log"
	"sort"
	"strings"
	"time"

	"recur/internal/bundles"
	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

// Fan-out. Every ready plan step runs in a CHILD WORKSPACE branched off this
// loop's branch — an isolated worktree converging (implement + independent
// review) under the parent's policy — and is merged back here the moment it
// settles. The parent never codes except to resolve a merge conflict; it
// integrates and then reviews the aggregate diff (board.go). Isolated workers
// per task, no shared tree, the planner re-adjusts as handoffs land, merge
// turbulence is accepted rather than serialized away, and no worker waits on
// its slowest sibling: a landed child unblocks its dependents right away.
//
// Ready steps are grouped by file overlap: steps that share a declared file
// ride together in one child so they cannot fight; steps with no declared
// files share one child. Children never fan out (one level only).
//
// A WAVE is the set of children started together — for the same reason (what
// had just landed). It is a record of a dispatch, not a barrier: its children
// merge independently and later waves start while it is still running.

// defaultFanOutPoll is how often a waiting parent re-checks its children.
const defaultFanOutPoll = 2 * time.Second

// fanOutChild / fanOutWave is the cockpit card payload (domain.FeedCardFanOut):
// the children of one wave and, once each has merged, its outcome. Live state
// while a child runs comes from the child's own workspace row on the client.
type fanOutChild struct {
	ID    string   `json:"id"`
	Title string   `json:"title"`
	Steps []string `json:"steps"`
	Mode  string   `json:"mode,omitempty"`  // chat child's chat mode chip
	State string   `json:"state,omitempty"` // "" while running; merged | failed | conflict: …
	Note  string   `json:"note,omitempty"`
}

type fanOutWave struct {
	Wave     int           `json:"wave"`
	Title    string        `json:"title,omitempty"` // chat-child card header; loops show "Wave N"
	After    []string      `json:"after,omitempty"` // parent steps whose landing started this wave
	Children []fanOutChild `json:"children"`
	Done     bool          `json:"done,omitempty"`
}

// waveChildren are this parent's children stamped with wave n, in store
// (creation) order; archived rows included — the store keeps merged children
// for history.
func (l *Loop) waveChildren(n int) []domain.Workspace {
	var out []domain.Workspace
	for _, c := range l.eng.Store.Workspaces() {
		if c.ParentID == l.ID && c.FanOut != nil && c.FanOut.Wave == n {
			out = append(out, c)
		}
	}
	return out
}

// emitWaveCard (re)publishes wave n's card under its stable ID, rebuilt from
// the children's rows so a restart, a later merge, or a resolved conflict all
// refresh the same card in place.
func (l *Loop) emitWaveCard(n int) {
	kids := l.waveChildren(n)
	if n <= 0 || len(kids) == 0 {
		return
	}
	w := fanOutWave{Wave: n, After: kids[0].FanOut.After, Done: true}
	merged := 0
	for _, c := range kids {
		row := fanOutChild{ID: c.ID, Title: c.Goal, Steps: c.ParentStepIDs, State: c.FanOut.Outcome, Note: c.FanOut.Note}
		if row.State == "" {
			w.Done = false
		}
		if row.State == "merged" {
			merged++
		}
		w.Children = append(w.Children, row)
	}
	data, err := json.Marshal(w)
	if err != nil {
		return
	}
	var b strings.Builder
	fmt.Fprintf(&b, "Wave %d: %d child workspace(s)", n, len(kids))
	if len(w.After) > 0 {
		fmt.Fprintf(&b, " after %s", html.EscapeString(strings.Join(w.After, ", ")))
	}
	if w.Done {
		fmt.Fprintf(&b, ", %d merged", merged)
	}
	b.WriteString(" — ")
	for i, c := range w.Children {
		if i > 0 {
			b.WriteString(" · ")
		}
		b.WriteString(html.EscapeString(strings.Join(c.Steps, ", ")))
	}
	l.eng.emitWorkspaceCard(l.ID, domain.FeedItem{
		ID: fmt.Sprintf("fanout-%s-%d", l.ID, n), AgentCode: "MA", Name: "Maestro",
		Text: b.String(), ChatCode: l.ChatCode, Card: domain.FeedCardFanOut, Data: string(data),
	})
}

// setChildOutcome records what the parent made of a settled child on the
// child's row (the wave card reads it) — before finishChild archives it.
func (l *Loop) setChildOutcome(childID, outcome, note string) {
	l.eng.Store.UpdateWorkspace(childID, func(w *domain.Workspace) {
		if w.FanOut == nil {
			w.FanOut = &domain.ChildFanOut{}
		}
		w.FanOut.Outcome = outcome
		w.FanOut.Note = truncate(strings.TrimSpace(note), 300)
	})
}

// childWave is the wave a child belongs to (0 if unknown).
func (l *Loop) childWave(childID string) int {
	if c, ok := l.eng.Store.Workspace(childID); ok && c.FanOut != nil {
		return c.FanOut.Wave
	}
	return 0
}

type fanOutResult struct {
	merged    int      // child branches merged this call
	conflicts []string // paths left conflicted by the last merge (merge in progress)
	stopped   bool     // the parent was stopped or cancelled while waiting
	noSlot    bool     // ready steps exist but no loop slot was free for a child
	handoffs  []string // merged children's done-notes ("S2: …"), for the replan turn
}

func (l *Loop) fanOutEligible() bool {
	return l != nil && l.planStructured && l.parentID == ""
}

// readySteps are pending steps whose dependencies are done/skipped, minus
// excluded child workspace ids (and any leftover step ids).
func readySteps(p domain.Plan, exclude map[string]bool) []domain.PlanStep {
	var out []domain.PlanStep
	for _, s := range p.Steps {
		if !p.Ready(s) || exclude[s.ID] || (s.Workspace != "" && exclude[s.Workspace]) {
			continue
		}
		out = append(out, s)
	}
	return out
}

// groupByFiles partitions steps into children: steps sharing any declared
// file join one group (transitively); steps with no files share one group.
// Groups keep plan order by their first step.
func groupByFiles(steps []domain.PlanStep) [][]domain.PlanStep {
	n := len(steps)
	parent := make([]int, n)
	for i := range parent {
		parent[i] = i
	}
	find := func(i int) int {
		for parent[i] != i {
			parent[i] = parent[parent[i]]
			i = parent[i]
		}
		return i
	}
	union := func(a, b int) { parent[find(a)] = find(b) }
	owner := map[string]int{}
	noFiles := -1
	for i, s := range steps {
		if len(s.Files) == 0 {
			if noFiles < 0 {
				noFiles = i
			} else {
				union(i, noFiles)
			}
			continue
		}
		for _, f := range s.Files {
			f = strings.TrimSpace(f)
			if j, ok := owner[f]; ok {
				union(i, j)
			} else {
				owner[f] = i
			}
		}
	}
	byRoot := map[int][]domain.PlanStep{}
	var order []int
	for i, s := range steps {
		r := find(i)
		if _, seen := byRoot[r]; !seen {
			order = append(order, r)
		}
		byRoot[r] = append(byRoot[r], s)
	}
	sort.Ints(order)
	out := make([][]domain.PlanStep, 0, len(order))
	for _, r := range order {
		out = append(out, byRoot[r])
	}
	return out
}

// inFlightChildren maps a child workspace id to the steps it owns (a previous
// wave, or a parent resumed mid-wait). Plan order.
func inFlightChildren(p domain.Plan) (ids []string, steps map[string][]domain.PlanStep) {
	steps = map[string][]domain.PlanStep{}
	for _, s := range p.Steps {
		id := strings.TrimSpace(s.Workspace)
		if s.Status != domain.StepDoing || id == "" {
			continue
		}
		if _, seen := steps[id]; !seen {
			ids = append(ids, id)
		}
		steps[id] = append(steps[id], s)
	}
	return ids, steps
}

// pendingConflictChild is a child whose merge is already in progress. Prefer
// the branch that owns MERGE_HEAD; otherwise a recorded conflict outcome.
func (l *Loop) pendingConflictChild(ctx context.Context) string {
	if l.git != nil && l.git.MergeHead(ctx, l.Worktree) {
		for _, c := range l.eng.Store.Workspaces() {
			if c.ParentID != l.ID || strings.TrimSpace(c.Branch) == "" {
				continue
			}
			if l.git.MergeHeadEquals(ctx, l.Worktree, c.Branch) {
				return c.ID
			}
		}
	}
	for _, c := range l.eng.Store.Workspaces() {
		if c.ParentID != l.ID || c.FanOut == nil {
			continue
		}
		if strings.HasPrefix(c.FanOut.Outcome, "conflict") {
			return c.ID
		}
	}
	return ""
}

// fanOutSteps drives the board until an event the caller handles: children
// handed off (the Maestro replans while the rest keep running), a merge
// conflicted (the parent resolves it), the parent was stopped, ready steps
// with no free slot and nothing running, or the board is idle (nothing in
// flight, nothing ready). Between events it is continuous: a settled child
// merges as soon as it settles and whatever its landing unblocked starts at
// once — no wave waits on its slowest member. In-flight children are read
// from the plan each pass (status doing + child id), so a call after an event
// or a restart picks up exactly where the board was.
func (l *Loop) fanOutSteps(ctx context.Context, exclude map[string]bool) fanOutResult {
	var res fanOutResult
	first := true
	for {
		ws, ok := l.eng.Store.Workspace(l.ID)
		if !ok || ws.Plan == nil {
			return res
		}
		ids, owned := inFlightChildren(*ws.Plan)
		if first {
			first = false
			// A resumed parent (restart, or re-entry after an event) picks its
			// interrupted children back up and refreshes the cards of the waves
			// still in flight — same ids, same numbers.
			l.resumeInterruptedChildren(ids)
			seen := map[int]bool{}
			for _, id := range ids {
				if n := l.childWave(id); n > 0 && !seen[n] {
					seen[n] = true
					l.emitWaveCard(n)
				}
			}
		}
		var settled []string
		if id := l.pendingConflictChild(ctx); id != "" {
			if _, ok := owned[id]; !ok {
				owned[id] = l.stepsOwnedBy(id)
			}
			settled = []string{id}
		} else {
			if ready := readySteps(*ws.Plan, exclude); len(ready) > 0 {
				free := l.eng.gov.FreeLoops()
				if free == 0 && len(ids) == 0 {
					res.noSlot = true
					return res
				}
				if free > 0 {
					groups := groupByFiles(ready)
					if len(groups) > free {
						groups = groups[:free]
					}
					waveNo := ws.FanOutWaves + 1
					after := append([]string(nil), l.boardUnblocked...)
					started := 0
					for _, g := range groups {
						id, err := l.eng.startStepChild(l, ws, g, waveNo, after)
						if err != nil {
							l.emitFeed("MA", "Maestro", fmt.Sprintf("Could not start a child workspace for %s: %s",
								html.EscapeString(stepIDs(g)), html.EscapeString(err.Error())))
							continue
						}
						ids = append(ids, id)
						owned[id] = g
						started++
					}
					if started > 0 {
						l.eng.Store.UpdateWorkspace(l.ID, func(w *domain.Workspace) { w.FanOutWaves = waveNo })
						l.boardUnblocked = nil
						l.emitWaveCard(waveNo)
					} else if len(ids) == 0 {
						return res // nothing could start and nothing runs: the board falls back
					}
				}
			}
			if len(ids) == 0 {
				return res // idle: nothing in flight, nothing ready
			}
			l.setBoardStatus(ids)
			var ok bool
			settled, ok = l.waitAnyChild(ctx, ids)
			if !ok {
				res.stopped = true
				return res
			}
		}
		for _, id := range settled {
			conflicts, merged, handoff, failed := l.mergeChild(ctx, id, owned[id])
			l.emitWaveCard(l.childWave(id))
			if merged {
				res.merged++
			}
			if merged && !failed {
				for _, s := range owned[id] {
					l.boardUnblocked = append(l.boardUnblocked, s.ID)
				}
			}
			if handoff != "" {
				res.handoffs = append(res.handoffs, stepIDs(owned[id])+": "+handoff)
			}
			if failed {
				exclude[id] = true // stable child id; step S1/S2 keys go stale after a replan
			}
			if len(conflicts) > 0 {
				res.conflicts = conflicts
				return res // other settled children merge on the next call
			}
		}
		if len(res.handoffs) > 0 {
			return res // the Maestro replans off these; the board resumes after
		}
	}
}

// setBoardStatus is the parent's status line while children run.
func (l *Loop) setBoardStatus(inflight []string) {
	running, needs := 0, 0
	for _, id := range inflight {
		if c, ok := l.eng.Store.Workspace(id); ok && c.State == domain.StateEscalated {
			needs++
		} else {
			running++
		}
	}
	done, total := 0, 0
	if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Plan != nil {
		for _, s := range ws.Plan.Steps {
			total++
			if s.Status == domain.StepDone || s.Status == domain.StepSkipped {
				done++
			}
		}
	}
	label := fmt.Sprintf("board: %d child workspace(s) running, %d of %d steps done", running, done, total)
	if needs > 0 {
		label = fmt.Sprintf("board: %d child workspace(s) need you, %d running, %d of %d steps done", needs, running, done, total)
	}
	l.setState(domain.StateCoding, label)
}

// waitAnyChild blocks at loop checkpoints until at least one child has
// settled, returning the settled ids in plan order. Returns ok=false when the
// parent was stopped or cancelled; a human Stop also stops the children so no
// orphan keeps burning agent slots.
func (l *Loop) waitAnyChild(ctx context.Context, ids []string) (settled []string, ok bool) {
	for {
		if !l.checkpoint(ctx) {
			l.stopRunningChildren(ids)
			return nil, false
		}
		settled = settled[:0]
		for _, id := range ids {
			if l.eng.childSettled(id) {
				settled = append(settled, id)
			}
		}
		if len(settled) > 0 {
			return settled, true
		}
		select {
		case <-ctx.Done():
			l.stopRunningChildren(ids)
			return nil, false
		case <-time.After(l.eng.fanOutPoll):
		}
	}
}

// childSettled reports whether a child's walk is over: no goroutine and a
// terminal or idle row. Queued, escalated, or mid-walk children are not
// settled — an escalated child waits for you while its siblings merge on.
func (e *Engine) childSettled(id string) bool {
	ws, ok := e.Store.Workspace(id)
	if !ok {
		return true
	}
	if e.runningLoopByID(id) != nil {
		return false
	}
	switch ws.State {
	case domain.StateStopped, domain.StateFailed, domain.StateMerged, domain.StatePROpen:
		return true
	case domain.StatePaused:
		return loopSettled(ws.State, ws.Phase) || ws.Phase == "loop refs missing" || ws.Phase == "phase missing"
	}
	return false
}

func stepIDs(steps []domain.PlanStep) string {
	ids := make([]string, 0, len(steps))
	for _, s := range steps {
		ids = append(ids, s.ID)
	}
	return strings.Join(ids, ", ")
}

// startStepChild creates and dispatches a child workspace for one step group:
// a loop derived from this loop's converge phase (guardForWorkspace), this
// loop's policy, base = this loop's
// branch, and a plan that scopes the child to its slice. The parent steps
// become doing and record the child's id; the child records its wave and
// what had just landed to start it (the wave card is rebuilt from that).
func (e *Engine) startStepChild(parent *Loop, pws domain.Workspace, steps []domain.PlanStep, wave int, after []string) (string, error) {
	title := steps[0].Title
	if len(steps) > 1 {
		title = fmt.Sprintf("%s (+%d more)", steps[0].Title, len(steps)-1)
	}
	// Hold dispatchMu across insert → parent metadata → dispatch so drainQueued
	// cannot promote an incomplete queued row (no ParentID/Plan yet) or steal
	// this child's slot and reset the parent's steps on errAlreadyDispatched.
	e.dispatchMu.Lock()
	defer e.dispatchMu.Unlock()
	rec, acquired, err := e.queueStartWorkspace(parent.ProjectID, truncate(title, 200), "",
		parent.Branch, parent.Autonomy, bundles.StepLoopID, parent.PolicyID, nil)
	if err != nil {
		return "", err
	}
	if e.childQueueHook != nil {
		e.childQueueHook()
	}
	childPlan := childStepPlan(pws, steps)
	rec.ParentID = parent.ID
	if ph, ok := parent.walkPhase(); ok {
		rec.ParentPhaseID = ph.ID
	}
	for _, s := range steps {
		rec.ParentStepIDs = append(rec.ParentStepIDs, s.ID)
	}
	rec.Plan = &childPlan
	rec.DefinitionOfDone = truncate(childPlan.Text(), planMaxPersistChars)
	rec.FanOut = &domain.ChildFanOut{Wave: wave, After: append([]string(nil), after...)}
	e.Store.UpdateWorkspace(rec.ID, func(w *domain.Workspace) {
		w.ParentID, w.ParentStepIDs, w.ParentPhaseID = rec.ParentID, rec.ParentStepIDs, rec.ParentPhaseID
		w.Plan, w.DefinitionOfDone = rec.Plan, rec.DefinitionOfDone
		w.FanOut = rec.FanOut
	})
	for _, s := range steps {
		parent.setStep(s.ID, domain.StepDoing, "", rec.ID)
	}
	if acquired {
		if err := e.dispatch(rec); err != nil {
			e.gov.ReleaseLoop()
			for _, s := range steps {
				parent.setStep(s.ID, domain.StepPending, "child dispatch failed: "+err.Error(), "")
			}
			return "", err
		}
	}
	return rec.ID, nil
}

// childStepPlan is the child's brief: its steps, their files, and the parent
// plan as context. Pre-approved so the child never gates on a human.
func childStepPlan(parent domain.Workspace, steps []domain.PlanStep) domain.Plan {
	var b strings.Builder
	b.WriteString("You are working a slice of a larger plan in your own workspace, branched from the parent's ")
	b.WriteString("branch. Do only the steps below. Sibling workspaces are working other steps at the same time, ")
	b.WriteString("so stay inside the files listed; if a step cannot be done without touching another file, do ")
	b.WriteString("the minimum and say so in your done notes.\n\nYour steps:\n")
	files := map[string]bool{}
	childSteps := make([]domain.PlanStep, 0, len(steps))
	for i, s := range steps {
		b.WriteString("- " + s.ID + ": " + s.Title + "\n")
		for _, f := range s.Files {
			files[f] = true
		}
		childSteps = append(childSteps, domain.PlanStep{
			ID: fmt.Sprintf("S%d", i+1), Title: s.ID + ": " + s.Title, Status: domain.StepPending, Files: s.Files,
		})
	}
	if len(files) > 0 {
		list := make([]string, 0, len(files))
		for f := range files {
			list = append(list, f)
		}
		sort.Strings(list)
		b.WriteString("Files you own: " + strings.Join(list, ", ") + "\n")
	}
	b.WriteString("\nParent task: " + strings.TrimSpace(parent.Goal) + "\n")
	if parent.Plan != nil {
		if sum := strings.TrimSpace(parent.Plan.Summary); sum != "" {
			b.WriteString("Parent plan: " + sum + "\n")
		}
		if body := strings.TrimSpace(parent.Plan.Body); body != "" {
			b.WriteString("\nParent plan detail (context only — do not work its other steps):\n")
			b.WriteString(body)
		}
	}
	summary := steps[0].Title
	if len(steps) > 1 {
		summary = fmt.Sprintf("%s (+%d more)", steps[0].Title, len(steps)-1)
	}
	return domain.Plan{
		Summary:  summary,
		Body:     b.String(),
		Steps:    childSteps,
		Source:   "loop-planner",
		Approved: true,
	}
}

// setStep updates one parent step's status/note/child link directly (the
// engine owns step transitions; this is the fan-out path, not an agent call).
func (l *Loop) setStep(stepID, status, note, childID string) {
	updated, ok := l.eng.Store.UpdateWorkspace(l.ID, func(w *domain.Workspace) {
		if w.Plan == nil {
			return
		}
		p := *w.Plan
		p.Steps = append([]domain.PlanStep(nil), p.Steps...)
		for i := range p.Steps {
			if p.Steps[i].ID != stepID {
				continue
			}
			p.Steps[i].Status = status
			if strings.TrimSpace(note) != "" {
				p.Steps[i].Note = truncate(strings.TrimSpace(note), 300)
			}
			if childID != "" {
				p.Steps[i].Workspace = childID
			}
		}
		w.Plan = &p
		w.DefinitionOfDone = truncate(p.Text(), planMaxPersistChars)
		if prog, ok := runProgress(w.State, w.Plan); ok {
			w.Progress = prog
		}
	})
	if ok {
		l.eng.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	}
}

// mergeChild merges a settled child's branch into this worktree and closes
// its steps. A child that failed or was stopped still has its partial commits
// merged (honest, reviewable work) but its steps stay pending with a note and
// are reported failed so the parent finishes them itself rather than spawning
// another child. Conflicts are returned with the merge left in progress.
func (l *Loop) mergeChild(ctx context.Context, childID string, steps []domain.PlanStep) (conflicts []string, merged bool, handoff string, failed bool) {
	child, ok := l.eng.Store.Workspace(childID)
	if !ok {
		for _, s := range steps {
			l.setStep(s.ID, domain.StepPending, "child workspace vanished — running here", "")
		}
		return nil, false, "", true
	}
	handoff = strings.TrimSpace(child.Handoff)
	if open := l.eng.Store.OpenFindings(child.ID); len(open) > 0 {
		// The child's review left findings it could not close (policy cap):
		// hand them up so the aggregate Reviewer knows where to look.
		var b strings.Builder
		fmt.Fprintf(&b, "%d unresolved finding(s) from the child's review:", len(open))
		for i, f := range open {
			if i == 5 {
				b.WriteString(" …")
				break
			}
			fmt.Fprintf(&b, " [%s] %s — %s;", f.Severity, domain.FindingLoc(f.File, f.Line, f.EndLine), truncate(f.Message, 120))
		}
		handoff = strings.TrimSpace(handoff + "\n" + b.String())
	}
	failed = childIncomplete(child)
	ids := stepIDs(steps)
	already := child.FanOut != nil && strings.HasPrefix(child.FanOut.Outcome, "conflict")
	if l.git != nil && l.git.MergeHead(ctx, l.Worktree) {
		if !l.git.MergeHeadEquals(ctx, l.Worktree, child.Branch) {
			return nil, false, "", false
		}
		conflicts = l.git.UnmergedPaths(ctx, l.Worktree)
		if len(conflicts) == 0 {
			conflicts = []string{"."}
		}
		if !already {
			note := handoff
			if note == "" {
				note = "merged from child workspace"
			}
			note += " (merge conflicts: " + strings.Join(conflicts, ", ") + ")"
			for _, s := range steps {
				l.setStep(s.ID, domain.StepDoing, note, child.ID)
			}
			l.setChildOutcome(child.ID, "conflict: "+strings.Join(conflicts, ", "), handoff)
		}
		return conflicts, true, handoff, failed
	}
	if already {
		l.closeMergedChild(child, steps, failed, handoff)
		return nil, true, handoff, failed
	}

	if err := l.preserveChildWork(ctx, child); err != nil {
		l.emitFeed("MA", "Maestro", fmt.Sprintf("Could not save uncommitted work in child <code>%s</code> — keeping the worktree (%s).",
			html.EscapeString(child.ID), html.EscapeString(err.Error())))
		for _, s := range steps {
			l.setStep(s.ID, domain.StepPending, "child work not saved: "+err.Error(), "")
		}
		l.setChildOutcome(child.ID, "failed", "preserve failed: "+err.Error())
		return nil, false, handoff, true
	}
	name, email, err := gitpkg.Identity(ctx, l.Worktree)
	if err == nil {
		msg := fmt.Sprintf("Merge %s from %s: %s", ids, child.Branch, truncate(steps[0].Title, 60))
		conflicts, err = l.git.MergeInto(ctx, l.Worktree, child.Branch, msg, name, email)
	}
	if err != nil {
		l.emitFeed("MA", "Maestro", fmt.Sprintf("Could not merge child branch <code>%s</code> (%s) — %s run here.",
			html.EscapeString(child.Branch), html.EscapeString(err.Error()), html.EscapeString(ids)))
		for _, s := range steps {
			l.setStep(s.ID, domain.StepPending, "child merge failed: "+err.Error(), "")
		}
		l.setChildOutcome(child.ID, "failed", "merge failed: "+err.Error())
		l.eng.finishChild(child.ID)
		return nil, false, handoff, true
	}
	if len(conflicts) > 0 {
		note := handoff
		if note == "" {
			note = "merged from child workspace"
		}
		note += " (merge conflicts: " + strings.Join(conflicts, ", ") + ")"
		for _, s := range steps {
			l.setStep(s.ID, domain.StepDoing, note, child.ID)
		}
		l.setChildOutcome(child.ID, "conflict: "+strings.Join(conflicts, ", "), handoff)
		l.emitFeed("MA", "Maestro", fmt.Sprintf("Merged %s from <code>%s</code> with %d conflicted file(s) — sending to the Coder.",
			html.EscapeString(ids), html.EscapeString(child.Branch), len(conflicts)))
		return conflicts, true, handoff, failed
	}
	l.closeMergedChild(child, steps, failed, handoff)
	return nil, true, handoff, failed
}

func (l *Loop) closeMergedChild(child domain.Workspace, steps []domain.PlanStep, failed bool, handoff string) {
	status, note := domain.StepDone, handoff
	if note == "" {
		note = "merged from child workspace"
	}
	outcome := "merged"
	if failed {
		status = domain.StepPending
		note = fmt.Sprintf("child %s — partial work merged; finish here. %s", child.State, note)
		outcome = "failed"
	}
	for _, s := range steps {
		l.setStep(s.ID, status, note, "")
	}
	l.setChildOutcome(child.ID, outcome, handoff)
	l.eng.finishChild(child.ID)
}

// preserveChildWork commits dirty files in a settled child's worktree so
// teardown cannot drop uncommitted edits (failed/stopped children never reach
// the engine commit in runCoder). A failure must not be ignored: mergeChild
// keeps the worktree until this succeeds.
func (l *Loop) preserveChildWork(ctx context.Context, child domain.Workspace) error {
	if strings.TrimSpace(child.WorktreePath) == "" || l.git == nil {
		return nil
	}
	name, email, err := gitpkg.Identity(ctx, child.WorktreePath)
	if err != nil {
		return fmt.Errorf("preserve child %s identity: %w", child.ID, err)
	}
	if _, err := l.git.Commit(ctx, child.WorktreePath, "WIP: uncommitted child work", name, email); err != nil {
		return fmt.Errorf("preserve child %s work: %w", child.ID, err)
	}
	return nil
}

func childIncomplete(ws domain.Workspace) bool {
	switch ws.State {
	case domain.StateFailed, domain.StateStopped:
		return true
	case domain.StatePaused:
		return ws.Phase == "loop refs missing" || ws.Phase == "phase missing"
	}
	return false
}

// stopRunningChildren stops the children a parent leaves behind (human Stop,
// a failed resolve turn, the board cap). NOT on app shutdown: Shutdown cancels
// the children's own contexts, Recover marks them interrupted, and the parent's
// resume re-dispatches them (resumeInterruptedChildren) — stopping them here
// would turn a restart into "children lost, parent redoes their steps".
func (l *Loop) stopRunningChildren(ids []string) {
	if l.eng.isShuttingDown() {
		return
	}
	for _, id := range ids {
		l.eng.Stop(id)
	}
}

// resumeInterruptedChildren re-dispatches in-flight children a restart left
// paused/interrupted (Recover). A child the human paused is left alone.
func (l *Loop) resumeInterruptedChildren(ids []string) {
	for _, id := range ids {
		c, ok := l.eng.Store.Workspace(id)
		if !ok || c.State != domain.StatePaused || c.Phase != phaseInterrupted {
			continue
		}
		if l.eng.runningLoopByID(id) != nil {
			continue
		}
		l.eng.Resume(id)
	}
}

func (l *Loop) stopInFlightChildren() {
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok || ws.Plan == nil {
		return
	}
	ids, _ := inFlightChildren(*ws.Plan)
	l.stopRunningChildren(ids)
}

// markConflictsResolved closes children whose merge conflicted once the
// parent's resolve turn concluded the merge (MERGE_HEAD gone).
func (l *Loop) markConflictsResolved() {
	waves := map[int]bool{}
	for _, c := range l.eng.Store.Workspaces() {
		if c.ParentID != l.ID || c.FanOut == nil || !strings.HasPrefix(c.FanOut.Outcome, "conflict") {
			continue
		}
		note := strings.TrimSpace(c.FanOut.Note + " (conflicts resolved by the parent)")
		failed := childIncomplete(c)
		l.closeMergedChild(c, l.stepsOwnedBy(c.ID), failed, note)
		waves[c.FanOut.Wave] = true
	}
	for n := range waves {
		l.emitWaveCard(n)
	}
}

// stepsOwnedBy is the parent's current plan steps whose Workspace is childID.
// A replan may have renumbered them, so the child's original ParentStepIDs are
// not a stable key.
func (l *Loop) stepsOwnedBy(childID string) []domain.PlanStep {
	ws, ok := l.eng.Store.Workspace(l.ID)
	if !ok || ws.Plan == nil {
		return nil
	}
	var steps []domain.PlanStep
	for _, s := range ws.Plan.Steps {
		if s.Workspace == childID {
			steps = append(steps, s)
		}
	}
	return steps
}

// finishChild marks a merged child and archives it (history kept, worktree
// torn down in the background). Best-effort: a refusal is logged, not fatal.
func (e *Engine) finishChild(id string) {
	updated, ok := e.Store.UpdateWorkspace(id, func(w *domain.Workspace) {
		w.State = domain.StateMerged
		w.StatusLabel = statusLabel(domain.StateMerged)
		w.Phase = "merged into parent"
		w.Group = domain.GroupReady
		w.Checkpoint = nil
	})
	if ok {
		e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	}
	if _, err := e.RemoveWorkspace(id); err != nil {
		log.Printf("fan-out: archive child %s: %v", id, err)
	}
}
