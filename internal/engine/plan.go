package engine

import (
	"encoding/json"
	"fmt"
	"html"
	"os"
	"path/filepath"
	"strings"

	"recur/internal/domain"
	"recur/internal/mcp"
	"recur/internal/store"
)

// Structured plan artifact: Plan-mode chats and
// the loop planner file plans via mcp__recur__submit_plan; the engine assigns
// step IDs, owns statuses, and mirrors Plan.Text() into DefinitionOfDone so
// every prose consumer (prompt builders, plan editor fallback) stays coherent.

const maxPlanAttachments = 20

// applySubmittedPlan persists a submit_plan call as the workspace's plan.
// source is derived from the filing agent: the Maestro (MA) is the loop
// planner, "AR" is the human (Plan panel edit), any other code is a Plan-mode
// chat.
func (e *Engine) applySubmittedPlan(workspaceID, agentCode string, sub mcp.PlanSubmission) string {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	source := planSource(agentCode)
	var prev []domain.PlanStep
	if ws.Plan != nil {
		prev = ws.Plan.Steps
	}
	steps := mergePlanSteps(prev, sub.Steps, sub.StepIDs, sub.StepMeta)
	atts, unknown := resolvePlanAttachments(workspaceID, sub.AttachmentIDs)
	plan := domain.Plan{
		Summary:     strings.TrimSpace(sub.Summary),
		Body:        strings.TrimSpace(sub.Body),
		Steps:       steps,
		Attachments: atts,
		Source:      source,
		ChatCode:    planAuthorChatCode(agentCode, ws.Plan),
	}
	capPlanForPersist(&plan)
	e.setWorkspacePlan(workspaceID, plan)
	author := "Agent"
	if source == "human" {
		author = "You"
	}
	e.emitPlanCard(workspaceID, agentCode, author, plan)
	e.touchWorkspaceActivity(workspaceID, "Filed a structured plan", e.now())
	msg := fmt.Sprintf("Plan recorded (%d steps).", len(steps))
	if n := len(atts); n > 0 {
		msg = fmt.Sprintf("Plan recorded (%d steps, %d attachment%s).", len(steps), n, pluralS(n))
	}
	if len(unknown) > 0 {
		msg += " Unknown attachment ids: " + strings.Join(unknown, ", ") + "."
	}
	msg += " The human reviews and approves it in the Plan panel."
	return msg
}

// planCardStep / planCardData is the cockpit plan card payload
// (domain.FeedCardPlan): the summary and the step checklist. Status is not
// carried — the card reads it live from the workspace's plan by step id.
type planCardStep struct {
	ID        string   `json:"id"`
	Title     string   `json:"title"`
	Files     []string `json:"files,omitempty"`
	DependsOn []string `json:"dependsOn,omitempty"`
}

type planCardData struct {
	Summary string         `json:"summary"`
	Steps   []planCardStep `json:"steps"`
	Source  string         `json:"source,omitempty"`
}

// emitPlanCard publishes a filed plan as one compact card (a new card per
// filing, so a replan shows as its own row). Text is the plain fallback.
func (e *Engine) emitPlanCard(workspaceID, agentCode, author string, plan domain.Plan) {
	d := planCardData{Summary: strings.TrimSpace(plan.Summary), Source: plan.Source}
	for _, s := range plan.Steps {
		d.Steps = append(d.Steps, planCardStep{ID: s.ID, Title: s.Title, Files: s.Files, DependsOn: s.DependsOn})
	}
	data, err := json.Marshal(d)
	if err != nil {
		return
	}
	e.emitWorkspaceCard(workspaceID, domain.FeedItem{
		ID: fmt.Sprintf("plan-%s-%d", workspaceID, e.now().UnixNano()), AgentCode: agentCode, Name: author,
		Text: fmt.Sprintf("Filed a structured plan: %d step(s) — %s", len(plan.Steps), html.EscapeString(truncate(plan.Summary, 160))),
		Card: domain.FeedCardPlan, Data: string(data),
	})
}

// planSource labels who filed a plan: a Plan-mode pair chat (PA*), the human
// (Plan panel, "AR"), or the loop's planner turn — whatever role code the
// bundle gave it (MA, PL, …). The planner turn adopts its own filing off this.
func planSource(agentCode string) string {
	switch {
	case isPairChatCode(agentCode):
		return "plan-chat"
	case agentCode == "AR":
		return "human"
	default:
		return "loop-planner"
	}
}

// planAuthorChatCode is the pair-chat tab that filed the plan (PA, PA2, …).
// Human / Maestro filings keep the previous code so a later panel edit does
// not drop the authoring chat for Run as loop Simplify inherit.
func planAuthorChatCode(agentCode string, prev *domain.Plan) string {
	if isPairChatCode(agentCode) {
		return normalizePairChatCode(agentCode)
	}
	if prev != nil {
		return strings.TrimSpace(prev.ChatCode)
	}
	return ""
}

func pluralS(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}

// resolvePlanAttachments looks up create_artifact ids and persisted chat-image
// ids. Unknown ids are returned separately so the tool result can name them
// without failing the whole submit.
func resolvePlanAttachments(workspaceID string, ids []string) (resolved []domain.PlanAttachment, unknown []string) {
	if len(ids) == 0 {
		return nil, nil
	}
	arts, _ := listArtifacts(workspaceID)
	byID := make(map[string]domain.Artifact, len(arts))
	for _, a := range arts {
		byID[a.ID] = a
	}
	seen := map[string]bool{}
	for _, raw := range ids {
		id := strings.TrimSpace(raw)
		if id == "" || seen[id] {
			continue
		}
		seen[id] = true
		if len(resolved)+len(unknown) >= maxPlanAttachments {
			break
		}
		if a, ok := byID[id]; ok {
			resolved = append(resolved, domain.PlanAttachment{
				ID: a.ID, Kind: string(a.Kind), Title: a.Title, Path: a.Path,
			})
			continue
		}
		if att, ok := lookupChatPlanAttachment(workspaceID, id); ok {
			resolved = append(resolved, att)
			continue
		}
		unknown = append(unknown, id)
	}
	return resolved, unknown
}

func lookupChatPlanAttachment(workspaceID, id string) (domain.PlanAttachment, bool) {
	id = sanitizeAttachmentID(id)
	if id == "" {
		return domain.PlanAttachment{}, false
	}
	root, err := store.DataDir()
	if err != nil {
		return domain.PlanAttachment{}, false
	}
	dir := filepath.Join(root, "workspaces", workspaceID, "attachments")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return domain.PlanAttachment{}, false
	}
	prefix := id + "."
	for _, ent := range entries {
		if ent.IsDir() {
			continue
		}
		name := ent.Name()
		if name != id && !strings.HasPrefix(name, prefix) {
			continue
		}
		kind := strings.ToLower(strings.TrimPrefix(filepath.Ext(name), "."))
		if kind == "jpg" {
			kind = "jpeg"
		}
		if kind == "" {
			kind = "image"
		}
		rel := filepath.ToSlash(filepath.Join("workspaces", workspaceID, "attachments", name))
		return domain.PlanAttachment{ID: id, Kind: kind, Title: name, Path: rel}, true
	}
	return domain.PlanAttachment{}, false
}

// firstParagraph is the headline taken from a prose plan (createPlan harvest).
// Leading markdown heading/list markers are stripped so "# Add dark mode"
// becomes a usable panel title, feed line, and Run-as-loop goal.
func firstParagraph(s string) string {
	s = strings.TrimSpace(s)
	if s == "" {
		return ""
	}
	for _, line := range strings.Split(s, "\n") {
		line = stripLeadingMarkdown(strings.TrimSpace(line))
		if line == "" {
			continue
		}
		return truncate(line, 400)
	}
	return truncate(s, 400)
}

func stripLeadingMarkdown(s string) string {
	s = strings.TrimSpace(s)
	s = strings.TrimLeft(s, "#")
	s = strings.TrimSpace(s)
	if len(s) >= 2 && (s[0] == '*' || s[0] == '-') && s[1] == ' ' {
		s = strings.TrimSpace(s[2:])
	}
	return s
}

// mergePlanSteps keeps status/note across a re-file. Match order: existing id
// (Plan panel retitle), then exact title (agent re-file / reorder). Ids in the
// result are always S1…Sn in list order so Plan.Text() ordinals ("1. …") match
// update_plan_step ids — inserting or reordering must not leave S3 as the first
// step.
func mergePlanSteps(prev []domain.PlanStep, titles, ids []string, meta []mcp.PlanStepMeta) []domain.PlanStep {
	if len(titles) == 0 {
		return nil
	}
	used := make([]bool, len(prev))
	byID := make(map[string]int, len(prev))
	for i, s := range prev {
		if s.ID != "" {
			byID[s.ID] = i
		}
	}
	byTitle := make(map[string][]int, len(prev))
	for i, s := range prev {
		byTitle[s.Title] = append(byTitle[s.Title], i)
	}
	// Pass 1: which previous step (if any) each title carries forward.
	src := make([]int, len(titles))
	for i, raw := range titles {
		t := strings.TrimSpace(raw)
		src[i] = -1
		if i < len(ids) {
			if j, ok := byID[strings.TrimSpace(ids[i])]; ok && !used[j] {
				src[i] = j
			}
		}
		if src[i] < 0 {
			for _, j := range byTitle[t] {
				if !used[j] {
					src[i] = j
					break
				}
			}
		}
		if src[i] >= 0 {
			used[src[i]] = true
		}
	}
	// Steps are renumbered S1..Sn in filed order, so an edge a retained step
	// carries forward must follow its target to the target's new id; an edge to
	// a step that was dropped goes with it. Otherwise reordering silently
	// rewires dependencies (S2 "after S1" refiled first would depend on itself).
	renumber := make(map[string]string, len(prev))
	for i, j := range src {
		if j >= 0 {
			renumber[prev[j].ID] = fmt.Sprintf("S%d", i+1)
		}
	}
	out := make([]domain.PlanStep, len(titles))
	for i, raw := range titles {
		t := strings.TrimSpace(raw)
		newID := fmt.Sprintf("S%d", i+1)
		if j := src[i]; j >= 0 {
			s := prev[j]
			s.ID = newID
			s.Title = t
			var deps []string
			for _, d := range s.DependsOn {
				if nd, ok := renumber[strings.TrimSpace(d)]; ok && nd != newID {
					deps = append(deps, nd)
				}
			}
			s.DependsOn = deps
			out[i] = s
		} else {
			out[i] = domain.PlanStep{ID: newID, Title: t, Status: domain.StepPending}
		}
		// A re-file in the object form ({title, files, depends_on}) is
		// authoritative for that step — including empty lists, which clear what
		// the previous plan (or the Plan panel) recorded. A bare title keeps it.
		if i < len(meta) && (meta[i].Explicit || len(meta[i].Files) > 0 || len(meta[i].DependsOn) > 0) {
			out[i].Files = append([]string(nil), meta[i].Files...)
			out[i].DependsOn = append([]string(nil), meta[i].DependsOn...)
		}
	}
	return out
}

// capPlanForPersist shrinks Body (or Summary) so Plan.Text() — body plus the
// live Steps/Attachments suffix — fits in planMaxPersistChars. The checklist
// is what Coders execute from; it must not be the first thing the cap drops.
func capPlanForPersist(p *domain.Plan) {
	for range 8 {
		extra := len(p.Text()) - planMaxPersistChars
		if extra <= 0 {
			return
		}
		trim := extra + 8 // extra slack for truncate's ellipsis rune
		if len(p.Body) > 256 {
			n := len(p.Body) - trim
			if n < 256 {
				n = 256
			}
			p.Body = truncate(strings.TrimSpace(p.Body), n)
			continue
		}
		if len(p.Summary) > 64 {
			n := len(p.Summary) - trim
			if n < 64 {
				n = 64
			}
			p.Summary = truncate(strings.TrimSpace(p.Summary), n)
			continue
		}
		return
	}
}

// applyPlanStepUpdate validates and applies an update_plan_step call. The
// engine owns the transition: unknown ids are refused, statuses were already
// vocabulary-checked at the MCP boundary.
func (e *Engine) applyPlanStepUpdate(workspaceID, agentCode, stepID, status, note string) string {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if ws.Plan == nil || len(ws.Plan.Steps) == 0 {
		return "Refused: this workspace has no structured plan steps."
	}
	found := false
	var title string
	updated, _ := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		if l.Plan == nil {
			return
		}
		p := *l.Plan
		p.Steps = append([]domain.PlanStep(nil), p.Steps...)
		for i := range p.Steps {
			if p.Steps[i].ID == stepID {
				p.Steps[i].Status = status
				if strings.TrimSpace(note) != "" {
					p.Steps[i].Note = truncate(strings.TrimSpace(note), 300)
				}
				title = p.Steps[i].Title
				found = true
				break
			}
		}
		if !found {
			return
		}
		l.Plan = &p
		l.DefinitionOfDone = truncate(p.Text(), planMaxPersistChars)
		if prog, ok := runProgress(l.State, l.Plan); ok {
			l.Progress = prog
		}
	})
	if !found {
		return fmt.Sprintf("Refused: no plan step with id %q.", stepID)
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	e.touchWorkspaceActivity(workspaceID, "Updated plan step "+stepID, e.now())
	return fmt.Sprintf("Step %s (%s) → %s.", stepID, truncate(title, 60), status)
}

// runProgress maps plan-step completion into the run's progress band
//: once structured steps exist, the execution
// window (coding/reviewing) tracks steps done instead of the hardcoded
// per-state constants — the ring is real. Planning floors at 0.15; verify/PR
// keep their constants (they sit above the band's ceiling).
func runProgress(state domain.WorkspaceState, p *domain.Plan) (float64, bool) {
	if p == nil || (state != domain.StateCoding && state != domain.StateReviewing) {
		return 0, false
	}
	frac, ok := p.Progress()
	if !ok {
		return 0, false
	}
	return 0.15 + 0.7*frac, true
}

// setWorkspacePlan writes a plan artifact + its prose mirror and publishes the
// update so the Plan panel refreshes live.
func (e *Engine) setWorkspacePlan(workspaceID string, plan domain.Plan) {
	updated, ok := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		p := plan
		capPlanForPersist(&p)
		l.Plan = &p
		l.DefinitionOfDone = truncate(p.Text(), planMaxPersistChars)
		if prog, ok := runProgress(l.State, l.Plan); ok {
			l.Progress = prog
		}
	})
	if !ok {
		return
	}
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
}

// adoptProsePlan files a summary-only plan artifact from free-form prose (no
// steps). Used when Cursor's createPlan tool carries the brief instead of
// mcp__recur__submit_plan — Recur cannot disable createPlan, so we harvest it
// into the Plan panel / DefinitionOfDone. Does not overwrite a plan that already
// has structured steps (submit_plan won).
func (e *Engine) adoptProsePlan(workspaceID, agentCode, body string) bool {
	body = strings.TrimSpace(body)
	if body == "" {
		return false
	}
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return false
	}
	if ws.Plan != nil && len(ws.Plan.Steps) > 0 {
		return false
	}
	source := planSource(agentCode)
	e.setWorkspacePlan(workspaceID, domain.Plan{
		Summary:  firstParagraph(body),
		Body:     body,
		Source:   source,
		ChatCode: planAuthorChatCode(agentCode, ws.Plan),
	})
	e.emitWorkspaceEvent(workspaceID, agentCode, "Agent",
		"Captured plan from createPlan (prose only — no structured steps).")
	return true
}

// SubmitHumanPlan writes a structured plan on the human's behalf (Plan panel
// edit; source=human). Steps are trimmed; empty ones dropped. stepIDs is
// aligned with steps (blank for a new row) so a retitle keeps status/note.
// A summary-only plan (no steps) is allowed — Cursor createPlan harvests land
// that way, and the human may keep editing them as prose until they add
// checklist steps. Existing attachments are kept (the panel does not re-file them).
func (e *Engine) SubmitHumanPlan(workspaceID, summary, body string, steps, stepIDs []string) string {
	clean := make([]string, 0, len(steps))
	ids := make([]string, 0, len(steps))
	for i, s := range steps {
		t := strings.TrimSpace(s)
		if t == "" {
			continue
		}
		clean = append(clean, t)
		id := ""
		if i < len(stepIDs) {
			id = strings.TrimSpace(stepIDs[i])
		}
		ids = append(ids, id)
	}
	if strings.TrimSpace(summary) == "" {
		return "Refused: the plan needs a summary."
	}
	var attIDs []string
	if ws, ok := e.Store.Workspace(workspaceID); ok && ws.Plan != nil {
		for _, a := range ws.Plan.Attachments {
			if id := strings.TrimSpace(a.ID); id != "" {
				attIDs = append(attIDs, id)
			}
		}
	}
	return e.applySubmittedPlan(workspaceID, "AR", mcp.PlanSubmission{
		Summary: summary, Body: body, Steps: clean, StepIDs: ids, AttachmentIDs: attIDs,
	})
}

// UpdateHumanPlanStep changes one step's status on the human's behalf (Plan
// panel checkbox) — the same validated engine path as the agent's
// update_plan_step.
func (e *Engine) UpdateHumanPlanStep(workspaceID, stepID, status string) string {
	switch status {
	case domain.StepPending, domain.StepDoing, domain.StepDone, domain.StepSkipped:
	default:
		return fmt.Sprintf("Refused: unknown step status %q.", status)
	}
	return e.applyPlanStepUpdate(workspaceID, "AR", stepID, status, "")
}

// ApprovePlan marks the workspace's structured plan approved (the Plan panel's
// Approve action). Returns a status string.
func (e *Engine) ApprovePlan(workspaceID string) string {
	ws, ok := e.Store.Workspace(workspaceID)
	if !ok {
		return "Refused: that workspace no longer exists."
	}
	if ws.Plan == nil {
		return "Refused: this workspace has no structured plan to approve."
	}
	if ws.Plan.Approved {
		return "The plan is already approved."
	}
	updated, _ := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		if l.Plan == nil {
			return
		}
		p := *l.Plan
		p.Approved = true
		l.Plan = &p
	})
	e.emitWorkspaceEvent(workspaceID, "AR", "You", "Approved the plan.")
	e.touchWorkspaceActivity(workspaceID, "Approved the plan", e.now())
	e.publishLoopUpdated(updated.ID, updated.State, updated.Phase, updated.Iteration)
	return "Plan approved."
}
