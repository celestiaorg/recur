package engine

import (
	"context"
	"fmt"
	"strings"

	"recur/internal/bundles"
)

func (l *Loop) walkPhase() (bundles.LoopPhase, bool) {
	if l == nil || l.eng == nil {
		return bundles.LoopPhase{}, false
	}
	l.mu.Lock()
	cur := l.currentPhase
	l.mu.Unlock()
	if strings.TrimSpace(cur.ID) != "" {
		return cur, true
	}
	id := ""
	if ws, ok := l.eng.Store.Workspace(l.ID); ok && ws.Checkpoint != nil {
		id = strings.TrimSpace(ws.Checkpoint.PhaseID)
	}
	if id == "" {
		return bundles.LoopPhase{}, false
	}
	return l.guard.Loop.PhaseByID(id)
}

func (l *Loop) phaseSlot(slot string) string {
	ph, ok := l.walkPhase()
	if !ok || ph.Prompts == nil {
		return ""
	}
	return strings.TrimSpace(ph.Prompts[slot])
}

func (l *Loop) phaseRoleID(slot string) string {
	if ph, ok := l.walkPhase(); ok && ph.Roles != nil {
		if id := strings.TrimSpace(ph.Roles[slot]); id != "" {
			return id
		}
	}
	if l.guard.Loop.Roles != nil {
		return strings.TrimSpace(l.guard.Loop.Roles[slot])
	}
	return ""
}

func (l *Loop) catalogRolePrompt(roleID string) string {
	roleID = strings.TrimSpace(roleID)
	if roleID == "" {
		return ""
	}
	cat := l.loopCatalog()
	r, ok := bundles.FindRole(cat.Roles, roleID)
	if !ok {
		return ""
	}
	return strings.TrimSpace(r.Prompt)
}

func (l *Loop) bundleMission() string {
	if l.guard.Loop.Prompts == nil {
		return ""
	}
	return strings.TrimSpace(l.guard.Loop.Prompts["mission"])
}

// bundleSystemPrompt is Role + the human's task/plan extras.
// Empty role body falls back to the hardcoded helper.
//
// Loop mission ([[loop]] prompt) is the conductor charter when the human
// talks. Do not paste it onto specialists — Cursor prepends the role block
// to the user turn, and a review-and-fix charter fights a read-only reviewer.
func (l *Loop) bundleSystemPrompt(roleID, fallback, task, planExtra string) string {
	body := l.catalogRolePrompt(roleID)
	if body == "" {
		return fallback
	}
	var b strings.Builder
	b.WriteString(body)
	task = strings.TrimSpace(task)
	if task != "" && task != l.bundleMission() {
		b.WriteString("\n\nTask: ")
		b.WriteString(task)
	}
	if strings.TrimSpace(planExtra) != "" {
		b.WriteString(planExtra)
	}
	return b.String()
}

// expandPhasePrompt runs ExpandComposer on the phase slot (or fallback).
// display is the [[prompt]] action label when the slot is a slash action;
// the model still gets prompt (the full wire).
//
// No slot: still unwrap the fallback (Linear ⟦raw⟧ regions) but return empty
// display so callers keep GoalDisplay. ExpandComposer's wrap fallback would
// otherwise set Display to the raw wire because out != raw.
func (l *Loop) expandPhasePrompt(ctx context.Context, slot, fallback string) (prompt, display string, err error) {
	raw := fallback
	fromSlot := false
	if s := l.phaseSlot(slot); s != "" {
		raw = s
		fromSlot = true
	}
	cat := l.loopCatalog()
	opts := expandOpts{worktree: l.Worktree, workspaceID: l.ID, ignoreDisabled: true}
	if len(cat.Prompts) > 0 || len(cat.Roles) > 0 {
		opts.catalog = &cat
	}
	res, err := l.eng.ExpandComposer(ctx, raw, opts)
	if err != nil {
		return "", "", err
	}
	if strings.TrimSpace(res.Prompt) == "" {
		return fallback, "", nil
	}
	if !fromSlot {
		return res.Prompt, "", nil
	}
	return res.Prompt, res.Display, nil
}

func (l *Loop) findingsThresholdBriefing() string {
	if l == nil {
		return ""
	}
	th := l.guard.Policy.FindingsThreshold
	if ph, ok := l.walkPhase(); ok {
		th = l.guard.phaseThreshold(ph)
	}
	return formatFindingsThreshold(th)
}

func formatFindingsThreshold(th bundles.FindingsThreshold) string {
	if th.AllZero {
		return "Phase findings threshold: every open finding blocks (all severities at 0). Do not file a ship-blocker as low."
	}
	if len(th.Levels) == 0 {
		return ""
	}
	var parts []string
	for _, lv := range bundles.SeverityLevels {
		if cap, ok := th.Levels[lv]; ok {
			parts = append(parts, fmt.Sprintf("%s ≤ %d", lv, cap))
		}
	}
	if len(parts) == 0 {
		return ""
	}
	return "Phase findings threshold (engine-enforced; named levels only): " + strings.Join(parts, ", ") + ". Do not file a ship-blocker as low to sneak it through."
}
