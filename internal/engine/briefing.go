package engine

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"recur/internal/domain"
	"recur/internal/store"
)

const (
	briefQuietAfter = 24 * time.Hour
	briefExcerptMax = 8
	briefLineMax    = 240
)

type briefReason struct {
	code   string
	rank   int
	now    string
	next   string
	action domain.BriefAction
}

type briefInputs struct {
	ws           domain.Workspace
	projectName  string
	agents       []domain.Agent
	headers      []domain.Session
	activity     int64
	now          time.Time
	perm         *domain.PermissionRequest
	question     *domain.UserQuestion
	escalation   *domain.Escalation
	pr           *domain.PR
	openFindings int
}

// now is the injectable clock used by Briefing watermarks.
func (e *Engine) now() time.Time {
	if e != nil && e.nowFn != nil {
		return e.nowFn()
	}
	return time.Now()
}

// BriefingSnapshot projects current work across workspaces.
func (e *Engine) BriefingSnapshot() domain.BriefingSnapshot {
	if e == nil || e.Store == nil {
		return domain.BriefingSnapshot{}
	}
	now := e.now()
	workspaces := e.Store.Workspaces()
	projects := e.Store.Projects()
	projName := map[string]string{}
	for _, p := range projects {
		name := strings.TrimSpace(p.Name)
		if name == "" {
			name = p.ID
		}
		projName[p.ID] = name
	}
	headers := e.Store.SessionHeadersByWorkspace()
	eventTS := e.Store.LatestEventTSByWorkspace()
	prs := e.Store.PRs()
	prByWS := map[string]*domain.PR{}
	prByID := map[string]*domain.PR{}
	for i := range prs {
		p := &prs[i]
		prByID[p.ID] = p
		if p.WorkspaceID != "" {
			prByWS[p.WorkspaceID] = p
		}
	}
	esc := map[string]*domain.Escalation{}
	for _, ws := range workspaces {
		if e, ok := e.Store.Escalation(ws.ID); ok {
			cp := e
			esc[ws.ID] = &cp
		}
	}
	permByWS := map[string]*domain.PermissionRequest{}
	for _, p := range e.PendingPermissions() {
		cp := p
		if permByWS[p.WorkspaceID] == nil {
			permByWS[p.WorkspaceID] = &cp
		}
	}
	qByWS := map[string]*domain.UserQuestion{}
	for _, q := range e.PendingAskUsers() {
		cp := q
		if qByWS[q.WorkspaceID] == nil {
			qByWS[q.WorkspaceID] = &cp
		}
	}

	var briefs []domain.WorkspaceBrief
	seenPR := map[string]bool{}
	for _, ws := range workspaces {
		if ws.Archived {
			continue
		}
		in := briefInputs{
			ws:           ws,
			projectName:  projName[ws.ProjectID],
			agents:       e.Store.Agents(ws.ID),
			headers:      headers[ws.ID],
			activity:     effectiveWorkspaceActivity(ws, headers[ws.ID], eventTS[ws.ID], prByWS[ws.ID]),
			now:          now,
			perm:         permByWS[ws.ID],
			question:     qByWS[ws.ID],
			escalation:   esc[ws.ID],
			pr:           prByWS[ws.ID],
			openFindings: e.Store.OpenFindingsCount(ws.ID),
		}
		if ws.IsPRChatHost() {
			if p := prByID[ws.HostPRID]; p != nil {
				in.pr = p
			}
		}
		b, ok := buildWorkspaceBrief(in)
		if !ok {
			continue
		}
		if b.PRID != "" {
			if seenPR[b.PRID] {
				continue
			}
			seenPR[b.PRID] = true
		}
		briefs = append(briefs, b)
	}
	sortBriefs(briefs)
	counts := domain.BriefingCounts{}
	for _, b := range briefs {
		switch b.Bucket {
		case domain.BriefBucketNeeds:
			counts.Needs++
		case domain.BriefBucketWorking:
			counts.Working++
		case domain.BriefBucketPickup:
			counts.Pickup++
		}
	}
	return domain.BriefingSnapshot{
		GeneratedAt: now.UnixMilli(),
		Counts:      counts,
		Workspaces:  briefs,
	}
}

// WorkspaceContext returns bounded coordinator detail for one live target.
func (e *Engine) WorkspaceContext(id string) (domain.WorkspaceContext, error) {
	id = strings.TrimSpace(id)
	if e == nil || e.Store == nil || id == "" {
		return domain.WorkspaceContext{}, fmt.Errorf("workspace not found")
	}
	ws, ok := e.Store.Workspace(id)
	if !ok || ws.Archived {
		return domain.WorkspaceContext{}, fmt.Errorf("workspace %q not found", id)
	}
	snap := e.BriefingSnapshot()
	var brief domain.WorkspaceBrief
	found := false
	for _, b := range snap.Workspaces {
		if b.WorkspaceID == id {
			brief = b
			found = true
			break
		}
	}
	if !found {
		in := briefInputs{
			ws:           ws,
			projectName:  ws.ProjectID,
			agents:       e.Store.Agents(ws.ID),
			now:          e.now(),
			openFindings: e.Store.OpenFindingsCount(ws.ID),
		}
		if p, ok := e.Store.Project(ws.ProjectID); ok && strings.TrimSpace(p.Name) != "" {
			in.projectName = p.Name
		}
		brief, _ = buildWorkspaceBrief(in)
		brief.WorkspaceID = ws.ID
		brief.Title = workspaceTitle(ws)
	}
	ctx := domain.WorkspaceContext{Brief: brief}
	if ws.Plan != nil {
		ctx.PlanSteps = append([]domain.PlanStep(nil), ws.Plan.Steps...)
	}
	for _, a := range e.Store.Agents(ws.ID) {
		ctx.Agents = append(ctx.Agents, domain.AgentSummary{
			Code: a.Code, Name: a.Name, Status: a.Status, Task: a.Task, Unseen: a.Unseen,
		})
	}
	if p, ok := permByWorkspace(e.PendingPermissions(), ws.ID); ok {
		ctx.OpenAsks = append(ctx.OpenAsks, domain.OpenAsk{
			Kind: "permission", ID: p.ID, AgentCode: p.AgentCode, Summary: p.Summary,
		})
	}
	if q, ok := questionByWorkspace(e.PendingAskUsers(), ws.ID); ok {
		ctx.OpenAsks = append(ctx.OpenAsks, domain.OpenAsk{
			Kind: "question", ID: q.ID, AgentCode: q.AgentCode, Summary: q.Question,
		})
	}
	if esc, ok := e.Store.Escalation(ws.ID); ok && strings.TrimSpace(esc.Answer) == "" {
		ctx.OpenAsks = append(ctx.OpenAsks, domain.OpenAsk{
			Kind: "escalation", ID: esc.ID, Summary: esc.Question,
		})
	}
	for _, sess := range e.Store.SessionsForLoop(ws.ID) {
		cfg := parseAgentConfig(sess.ConfigJSON)
		if cfg.Code == "" {
			continue
		}
		ctx.RecentChats = append(ctx.RecentChats, domain.ChatHeader{
			Code: cfg.Code, Title: sessionTitle(cfg, sess), Mode: cfg.Mode,
			UpdatedAt: sess.UpdatedAt, Unseen: agentUnseen(e.Store.Agents(ws.ID), cfg.Code),
		})
		if len(ctx.RecentChats) >= 6 {
			break
		}
	}
	if prs := e.Store.PRs(); true {
		for i := range prs {
			if prs[i].WorkspaceID == ws.ID {
				p := prs[i]
				ctx.PR = &domain.PRSummary{
					ID: p.ID, Number: p.Number, Title: p.Title,
					State: p.State, ReviewDecision: p.ReviewDecision,
				}
				break
			}
		}
	}
	if sess, ok := e.Store.SessionForLoopAgent(ws.ID, brief.LatestChatCode); ok || brief.LatestChatCode == "" {
		if !ok {
			if list := e.Store.SessionsForLoop(ws.ID); len(list) > 0 {
				sess = list[len(list)-1]
				ok = true
			}
		}
		if ok {
			ctx.Conversation = conversationExcerpt(e.Store, sess.ID)
		}
	}
	return ctx, nil
}

func permByWorkspace(list []domain.PermissionRequest, id string) (domain.PermissionRequest, bool) {
	for _, p := range list {
		if p.WorkspaceID == id {
			return p, true
		}
	}
	return domain.PermissionRequest{}, false
}

func questionByWorkspace(list []domain.UserQuestion, id string) (domain.UserQuestion, bool) {
	for _, q := range list {
		if q.WorkspaceID == id {
			return q, true
		}
	}
	return domain.UserQuestion{}, false
}

func agentUnseen(agents []domain.Agent, code string) bool {
	for _, a := range agents {
		if a.Code == code {
			return a.Unseen
		}
	}
	return false
}

func conversationExcerpt(st *store.Store, sessionID string) []domain.ConversationLine {
	if st == nil || sessionID == "" {
		return nil
	}
	msgs, err := st.SessionTranscript(sessionID)
	if err != nil {
		return nil
	}
	var out []domain.ConversationLine
	for i := len(msgs) - 1; i >= 0 && len(out) < briefExcerptMax; i-- {
		m := msgs[i]
		if m.Role != "user" && m.Role != "assistant" {
			continue
		}
		text := strings.TrimSpace(messagePlainText(m))
		if text == "" {
			continue
		}
		out = append(out, domain.ConversationLine{
			Role: m.Role, Text: truncate(text, briefLineMax), At: m.CreatedAt,
		})
	}
	for i, j := 0, len(out)-1; i < j; i, j = i+1, j-1 {
		out[i], out[j] = out[j], out[i]
	}
	return out
}

func messagePlainText(m domain.Message) string {
	var b strings.Builder
	for _, c := range m.Content {
		if c.Type != domain.BlockText {
			continue
		}
		var p domain.TextPayload
		if json.Unmarshal(c.ContentJSON, &p) != nil {
			continue
		}
		t := strings.TrimSpace(p.Text)
		if t == "" && p.DisplayText != "" {
			t = strings.TrimSpace(p.DisplayText)
		}
		if t == "" {
			continue
		}
		if b.Len() > 0 {
			b.WriteByte('\n')
		}
		b.WriteString(t)
	}
	return b.String()
}

// MarkWorkspaceViewed records that the visible shell rendered this workspace
// through seenThroughActivityAt. LastViewedAt is now; SeenActivityAt only
// advances to the value the UI actually showed (max, so a racing event stays new).
func (e *Engine) MarkWorkspaceViewed(workspaceID string, seenThroughActivityAt int64) {
	if e == nil || e.Store == nil || strings.TrimSpace(workspaceID) == "" {
		return
	}
	now := e.now().UnixMilli()
	_, ok := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		if now > l.LastViewedAt {
			l.LastViewedAt = now
		}
		if seenThroughActivityAt > l.SeenActivityAt {
			l.SeenActivityAt = seenThroughActivityAt
		}
	})
	if ok {
		e.publishBriefingInvalidated(workspaceID)
	}
}

// touchWorkspaceActivity updates Last + LastActivityAt monotonically and
// invalidates Briefing. Call only at meaningful boundaries.
func (e *Engine) touchWorkspaceActivity(workspaceID, summary string, at time.Time) {
	if e == nil || e.Store == nil || strings.TrimSpace(workspaceID) == "" {
		return
	}
	if at.IsZero() {
		at = e.now()
	}
	ms := at.UnixMilli()
	summary = strings.TrimSpace(summary)
	changed := false
	_, ok := e.Store.UpdateWorkspace(workspaceID, func(l *domain.Workspace) {
		if ms > l.LastActivityAt {
			l.LastActivityAt = ms
			changed = true
		}
		if summary != "" && summary != l.Last {
			l.Last = truncate(summary, 160)
			changed = true
		}
	})
	if ok && changed {
		e.publishBriefingInvalidated(workspaceID)
	}
}

func (e *Engine) publishBriefingInvalidated(workspaceID string) {
	if e == nil || e.Bus == nil {
		return
	}
	data := map[string]any{}
	if workspaceID != "" {
		data["workspaceId"] = workspaceID
	}
	e.Bus.Publish(Event{Name: "briefing.updated", Data: data})
}

func effectiveWorkspaceActivity(ws domain.Workspace, headers []domain.Session, eventTS int64, pr *domain.PR) int64 {
	ms := ws.LastActivityAt
	if ms > 0 {
		return ms
	}
	for _, s := range headers {
		if s.UpdatedAt > ms {
			ms = s.UpdatedAt
		}
	}
	if eventTS > ms {
		ms = eventTS
	}
	if pr != nil {
		if t := parseRFC3339Milli(pr.UpdatedAt); t > ms {
			ms = t
		}
	}
	return ms
}

func parseRFC3339Milli(s string) int64 {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0
	}
	t, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t, err = time.Parse(time.RFC3339Nano, s)
		if err != nil {
			return 0
		}
	}
	return t.UnixMilli()
}

func buildWorkspaceBrief(in briefInputs) (domain.WorkspaceBrief, bool) {
	ws := in.ws
	reasons := collectBriefReasons(in)
	if len(reasons) == 0 && !includeIdleMain(in) {
		return domain.WorkspaceBrief{}, false
	}
	if settledEmpty(in) {
		return domain.WorkspaceBrief{}, false
	}
	primary := reasons[0]
	if len(reasons) == 0 {
		return domain.WorkspaceBrief{}, false
	}
	bucket := bucketFor(primary.code, in)
	if bucket == "" {
		return domain.WorkspaceBrief{}, false
	}
	codes := make([]string, 0, len(reasons))
	var secondary []domain.BriefAction
	seenAct := map[string]bool{primary.action.Target: true}
	for _, r := range reasons {
		codes = append(codes, r.code)
		if r.action.Target == "" || seenAct[r.action.Target] {
			continue
		}
		seenAct[r.action.Target] = true
		secondary = append(secondary, r.action)
	}
	title := workspaceTitle(ws)
	if ws.IsPRChatHost() && in.pr != nil && strings.TrimSpace(in.pr.Title) != "" {
		title = in.pr.Title
	}
	planDone, planTotal := planProgress(ws.Plan)
	latestCode, latestTitle := latestChat(in.headers, in.agents)
	activeN, activeTask := activeAgentSummary(in.agents)
	b := domain.WorkspaceBrief{
		WorkspaceID:    ws.ID,
		ProjectID:      ws.ProjectID,
		ProjectName:    in.projectName,
		Title:          title,
		Branch:         ws.Branch,
		Bucket:         bucket,
		Reasons:        codes,
		Now:            primary.now,
		Next:           primary.next,
		LastActivityAt: in.activity,
		LastViewedAt:   ws.LastViewedAt,
		SeenActivityAt: ws.SeenActivityAt,
		// "Changed since you looked" needs a prior look: a never-viewed
		// workspace (SeenActivityAt zero) must not raise the return-to-work
		// cue off its own creation event.
		ChangedSinceViewed: ws.SeenActivityAt > 0 && in.activity > ws.SeenActivityAt,
		QuietAge:           quietAge(in.activity, in.now),
		LastSummary:        strings.TrimSpace(ws.Last),
		State:              string(ws.State),
		Phase:              ws.Phase,
		PlanSummary:        planHeadline(ws.Plan),
		PlanDone:           planDone,
		PlanTotal:          planTotal,
		ActiveAgents:       activeN,
		ActiveAgentTask:    activeTask,
		LatestChatCode:     latestCode,
		LatestChatTitle:    latestTitle,
		DiffAdd:            ws.Diff.Add,
		DiffDel:            ws.Diff.Del,
		IsPRHost:           ws.IsPRChatHost(),
		Primary:            primary.action,
		Secondary:          secondary,
	}
	if in.pr != nil {
		b.PRID = in.pr.ID
		b.PRNumber = in.pr.Number
	}
	return b, true
}

func includeIdleMain(in briefInputs) bool {
	return false
}

func settledEmpty(in briefInputs) bool {
	ws := in.ws
	switch ws.State {
	case domain.StateMerged, domain.StateStopped:
		if in.perm == nil && in.question == nil && !hasUnseenChat(in.agents) && !prNeedsAttention(in.pr) {
			return true
		}
	}
	if ws.IsMain && !hasMeaningfulWork(in) {
		return true
	}
	return false
}

func hasMeaningfulWork(in briefInputs) bool {
	if in.perm != nil || in.question != nil || in.escalation != nil {
		return true
	}
	if hasUnseenChat(in.agents) || activeAgentCount(in.agents) > 0 {
		return true
	}
	if in.ws.Diff.Add+in.ws.Diff.Del > 0 {
		return true
	}
	if in.ws.Plan != nil && len(in.ws.Plan.Steps) > 0 && !planAllResolved(in.ws.Plan) {
		return true
	}
	if len(in.headers) > 0 {
		return true
	}
	switch in.ws.State {
	case domain.StateQueued, domain.StatePlanning, domain.StateCoding, domain.StateReviewing,
		domain.StateVerifying, domain.StateEscalated,
		domain.StateFailed, domain.StatePROpen:
		return true
	}
	return false
}

func collectBriefReasons(in briefInputs) []briefReason {
	var out []briefReason
	ws := in.ws
	if in.perm != nil {
		tool := strings.TrimSpace(in.perm.Tool)
		if tool == "" {
			tool = "a tool"
		}
		out = append(out, briefReason{
			code: domain.BriefReasonPermission, rank: 0,
			now:  fmt.Sprintf("An agent is waiting to use %s", tool),
			next: "Review the permission",
			action: domain.BriefAction{
				Label: "Review", Target: chatOrWorkspaceTarget(ws.ID, in.perm.AgentCode), Kind: domain.BriefActionReview,
			},
		})
	}
	if in.question != nil {
		q := truncate(strings.TrimSpace(in.question.Question), 140)
		chatName := chatLabel(in.agents, in.headers, in.question.AgentCode)
		out = append(out, briefReason{
			code: domain.BriefReasonQuestion, rank: 1,
			now:  waitingChatNow(chatName),
			next: q,
			action: domain.BriefAction{
				Label: "Answer", Target: "question:" + in.question.ID, Kind: domain.BriefActionAnswer,
			},
		})
	}
	if in.escalation != nil && strings.TrimSpace(in.escalation.Answer) == "" {
		out = append(out, briefReason{
			code: domain.BriefReasonEscalation, rank: 2,
			now:  "Run is paused on a decision",
			next: truncate(strings.TrimSpace(in.escalation.Question), 140),
			action: domain.BriefAction{
				Label: "Answer", Target: loopChatTarget(ws), Kind: domain.BriefActionAnswer,
			},
		})
	}
	if ws.State == domain.StateFailed {
		if ws.Retryable {
			out = append(out, briefReason{
				code: domain.BriefReasonRetryableFailure, rank: 3,
				now:  "The loop stopped at a checkpoint",
				next: "Continue in the loop chat",
				action: domain.BriefAction{
					Label: "Open", Target: loopChatTarget(ws), Kind: domain.BriefActionOpen,
				},
			})
		} else {
			reason := strings.TrimSpace(ws.LastError)
			if reason == "" {
				reason = "the persisted error"
			}
			out = append(out, briefReason{
				code: domain.BriefReasonFailure, rank: 4,
				now:  "Run failed: " + truncate(reason, 120),
				next: "Open the loop chat",
				action: domain.BriefAction{
					Label: "Open", Target: loopChatTarget(ws), Kind: domain.BriefActionOpen,
				},
			})
		}
	}
	if ws.Plan != nil && !ws.Plan.Approved && (strings.TrimSpace(ws.Plan.Body) != "" || len(ws.Plan.Steps) > 0) {
		out = append(out, briefReason{
			code: domain.BriefReasonPlanApproval, rank: 5,
			now:  "Plan is ready but not approved",
			next: "Review and approve the plan",
			action: domain.BriefAction{
				Label: "Review plan", Target: "workspace:" + ws.ID + ":plan", Kind: domain.BriefActionApprove,
			},
		})
	}
	if in.openFindings > 0 && !isActiveRun(ws.State) {
		nowTxt := "1 open finding"
		if in.openFindings != 1 {
			nowTxt = fmt.Sprintf("%d open findings", in.openFindings)
		}
		out = append(out, briefReason{
			code: domain.BriefReasonFindings, rank: 6,
			now:  nowTxt,
			next: "Review findings",
			action: domain.BriefAction{
				Label: "Review", Target: "workspace:" + ws.ID + ":findings", Kind: domain.BriefActionReview,
			},
		})
	}
	if prNeedsAttention(in.pr) {
		label := "PR is ready for review"
		if in.pr.ReviewDecision == domain.ReviewStateChangesRequested ||
			strings.Contains(strings.ToLower(in.pr.State), "change") {
			label = "PR requests changes"
		}
		out = append(out, briefReason{
			code: domain.BriefReasonPRAttention, rank: 7,
			now:  label,
			next: "Review the PR",
			action: domain.BriefAction{
				Label: "Review", Target: "pr:" + in.pr.ID, Kind: domain.BriefActionReview,
			},
		})
	}
	if code, name := firstUnseenChat(in.agents, in.headers); code != "" {
		out = append(out, briefReason{
			code: domain.BriefReasonUnseenChat, rank: 8,
			now:  unreadChatNow(name),
			next: "Read the response",
			action: domain.BriefAction{
				Label: "Read", Target: "chat:" + ws.ID + ":" + code, Kind: domain.BriefActionOpen,
			},
		})
	}
	if n, task, code := firstActiveAgent(in.agents); n > 0 {
		nowTxt := task
		if nowTxt == "" {
			nowTxt = statusNow(ws)
		} else {
			nowTxt = fmt.Sprintf("%s is %s", agentName(in.agents, code), task)
		}
		next := nextPlanStep(ws.Plan)
		if next == "" {
			next = "Open the chat"
		}
		out = append(out, briefReason{
			code: domain.BriefReasonActiveAgent, rank: 9,
			now:  nowTxt,
			next: next,
			action: domain.BriefAction{
				Label: "Open", Target: agentOrLoopChatTarget(ws, code), Kind: domain.BriefActionOpen,
			},
		})
	} else if isActiveRun(ws.State) {
		next := nextPlanStep(ws.Plan)
		if next == "" {
			next = "Open the loop chat"
		}
		out = append(out, briefReason{
			code: domain.BriefReasonActiveAgent, rank: 9,
			now:  statusNow(ws),
			next: next,
			action: domain.BriefAction{
				Label: "Open", Target: loopChatTarget(ws), Kind: domain.BriefActionOpen,
			},
		})
	}
	if ws.Plan != nil && len(ws.Plan.Steps) > 0 && !planAllResolved(ws.Plan) {
		done, total := planProgress(ws.Plan)
		out = append(out, briefReason{
			code: domain.BriefReasonUnfinishedPlan, rank: 11,
			now:  fmt.Sprintf("%d of %d plan steps are complete", done, total),
			next: firstOpenStep(ws.Plan),
			action: domain.BriefAction{
				Label: "Continue", Target: "workspace:" + ws.ID + ":plan", Kind: domain.BriefActionContinue,
			},
		})
	}
	if ws.Diff.Add+ws.Diff.Del > 0 && !isActiveRun(ws.State) {
		out = append(out, briefReason{
			code: domain.BriefReasonDirtyIdle, rank: 12,
			now:  "Branch has unlanded changes",
			next: "Review the unlanded changes",
			action: domain.BriefAction{
				Label: "Review", Target: "workspace:" + ws.ID + ":review", Kind: domain.BriefActionReview,
			},
		})
	}
	if code, title := latestChat(in.headers, in.agents); code != "" && title != "" &&
		!isActiveRun(ws.State) && ws.State != domain.StateFailed {
		out = append(out, briefReason{
			code: domain.BriefReasonRecentChat, rank: 13,
			now:  continueChatNow(title),
			next: "Continue that chat",
			action: domain.BriefAction{
				Label: "Continue", Target: "chat:" + ws.ID + ":" + code, Kind: domain.BriefActionContinue,
			},
		})
	}
	sortReasons(out)
	return out
}

func sortReasons(rs []briefReason) {
	for i := 0; i < len(rs); i++ {
		for j := i + 1; j < len(rs); j++ {
			if rs[j].rank < rs[i].rank {
				rs[i], rs[j] = rs[j], rs[i]
			}
		}
	}
}

func bucketFor(code string, in briefInputs) string {
	switch code {
	case domain.BriefReasonPermission, domain.BriefReasonQuestion, domain.BriefReasonEscalation,
		domain.BriefReasonRetryableFailure, domain.BriefReasonFailure, domain.BriefReasonPlanApproval,
		domain.BriefReasonFindings, domain.BriefReasonPRAttention, domain.BriefReasonUnseenChat:
		return domain.BriefBucketNeeds
	case domain.BriefReasonActiveAgent:
		return domain.BriefBucketWorking
	case domain.BriefReasonUnfinishedPlan, domain.BriefReasonDirtyIdle,
		domain.BriefReasonRecentChat:
		return domain.BriefBucketPickup
	}
	if isActiveRun(in.ws.State) || activeAgentCount(in.agents) > 0 {
		return domain.BriefBucketWorking
	}
	return ""
}

func isActiveRun(st domain.WorkspaceState) bool {
	switch st {
	case domain.StateQueued, domain.StatePlanning, domain.StateCoding, domain.StateReviewing,
		domain.StateVerifying:
		return true
	}
	return false
}

func prNeedsAttention(p *domain.PR) bool {
	if p == nil || strings.EqualFold(p.State, "Merged") || strings.EqualFold(p.State, "Closed") {
		return false
	}
	if p.ReviewRequested {
		return true
	}
	if p.ReviewDecision == domain.ReviewStateChangesRequested {
		return true
	}
	st := strings.ToLower(p.State)
	return strings.Contains(st, "review") || strings.Contains(st, "ready")
}

func hasUnseenChat(agents []domain.Agent) bool {
	for _, a := range agents {
		if a.Unseen && isChatAttentionCode(a.Code) {
			return true
		}
	}
	return false
}

func firstUnseenChat(agents []domain.Agent, headers []domain.Session) (code, name string) {
	for _, a := range agents {
		if a.Unseen && isChatAttentionCode(a.Code) {
			return a.Code, chatLabel(agents, headers, a.Code)
		}
	}
	return "", ""
}

func activeAgentCount(agents []domain.Agent) int {
	n := 0
	for _, a := range agents {
		if isLiveAgentStatus(a.Status) {
			n++
		}
	}
	return n
}

func firstActiveAgent(agents []domain.Agent) (n int, task, code string) {
	for _, a := range agents {
		if isLiveAgentStatus(a.Status) {
			n++
			if code == "" {
				code = a.Code
				task = strings.TrimSpace(a.Task)
				if task == "" {
					task = a.Status
				}
			}
		}
	}
	return n, task, code
}

func activeAgentSummary(agents []domain.Agent) (n int, task string) {
	n, task, _ = firstActiveAgent(agents)
	return n, task
}

func workspaceTitle(ws domain.Workspace) string {
	if s := strings.TrimSpace(ws.Short); s != "" {
		return s
	}
	if s := strings.TrimSpace(ws.Goal); s != "" {
		return truncate(s, 80)
	}
	return ws.ID
}

func statusNow(ws domain.Workspace) string {
	if p := strings.TrimSpace(ws.Phase); p != "" {
		return "Run is " + p
	}
	if s := strings.TrimSpace(ws.StatusLabel); s != "" {
		return s
	}
	if ws.State != "" {
		return "Run is " + string(ws.State)
	}
	return "Work is in progress"
}

func planHeadline(p *domain.Plan) string {
	if p == nil {
		return ""
	}
	return strings.TrimSpace(p.Summary)
}

func planProgress(p *domain.Plan) (done, total int) {
	if p == nil {
		return 0, 0
	}
	total = len(p.Steps)
	for _, s := range p.Steps {
		if s.Status == domain.StepDone || s.Status == domain.StepSkipped {
			done++
		}
	}
	return done, total
}

func planAllResolved(p *domain.Plan) bool {
	if p == nil || len(p.Steps) == 0 {
		return true
	}
	for _, s := range p.Steps {
		if s.Status != domain.StepDone && s.Status != domain.StepSkipped {
			return false
		}
	}
	return true
}

func nextPlanStep(p *domain.Plan) string {
	if p == nil {
		return ""
	}
	for _, s := range p.Steps {
		if s.Status == domain.StepDoing {
			return s.Title
		}
	}
	return firstOpenStep(p)
}

func firstOpenStep(p *domain.Plan) string {
	if p == nil {
		return ""
	}
	for _, s := range p.Steps {
		if s.Status == domain.StepDoing || s.Status == domain.StepPending {
			return s.Title
		}
	}
	return ""
}

func latestChat(headers []domain.Session, agents []domain.Agent) (code, title string) {
	var best domain.Session
	bestAt := int64(-1)
	for _, s := range headers {
		cfg := parseAgentConfig(s.ConfigJSON)
		if cfg.Code == "" || !isChatAttentionCode(cfg.Code) {
			continue
		}
		if s.UpdatedAt >= bestAt {
			bestAt = s.UpdatedAt
			best = s
			code = cfg.Code
			title = sessionTitle(cfg, s)
		}
	}
	if code != "" {
		return code, title
	}
	_ = best
	for _, a := range agents {
		if isChatAttentionCode(a.Code) {
			return a.Code, a.Name
		}
	}
	return "", ""
}

func sessionTitle(cfg agentConfig, s domain.Session) string {
	if t := strings.TrimSpace(cfg.Title); t != "" {
		return t
	}
	if t := strings.TrimSpace(cfg.Name); t != "" {
		return t
	}
	return cfg.Code
}

// chatNoun is the chat's identity in Now copy. Generic tab labels collapse to
// "chat"; real tab titles are quoted so they read as a chat, not a status line.
func chatNoun(name string) string {
	name = strings.TrimSpace(name)
	switch strings.ToLower(name) {
	case "", "new chat", "chat", "agent":
		return "chat"
	case "maestro", "loop":
		return name
	}
	if isChatAttentionCode(name) {
		return "chat"
	}
	return "“" + truncate(name, 80) + "”"
}

func continueChatNow(title string) string {
	n := chatNoun(title)
	if n == "chat" {
		return "Continue the latest chat"
	}
	return "Continue the " + n + " chat"
}

func unreadChatNow(name string) string {
	n := chatNoun(name)
	if n == "chat" {
		return "New reply in chat"
	}
	return "New reply in " + n
}

func waitingChatNow(name string) string {
	n := chatNoun(name)
	if n == "chat" {
		return "A chat is waiting for an answer"
	}
	return n + " is waiting for an answer"
}

func parseAgentConfig(raw string) agentConfig {
	var c agentConfig
	_ = json.Unmarshal([]byte(raw), &c)
	return c
}

func chatLabel(agents []domain.Agent, headers []domain.Session, code string) string {
	if code == "" {
		return "Chat"
	}
	for _, a := range agents {
		if a.Code == code {
			if t := strings.TrimSpace(a.Name); t != "" {
				return t
			}
			break
		}
	}
	for _, s := range headers {
		cfg := parseAgentConfig(s.ConfigJSON)
		if cfg.Code == code {
			if t := sessionTitle(cfg, s); t != "" {
				return t
			}
		}
	}
	if code == maestroAgentCode {
		return "Maestro"
	}
	return code
}

func agentName(agents []domain.Agent, code string) string {
	for _, a := range agents {
		if a.Code == code && strings.TrimSpace(a.Name) != "" {
			return a.Name
		}
	}
	return code
}

func chatOrWorkspaceTarget(workspaceID, code string) string {
	if isChatAttentionCode(code) {
		return "chat:" + workspaceID + ":" + code
	}
	return "workspace:" + workspaceID
}

func loopChatTarget(ws domain.Workspace) string {
	code := loopOwnerCode(ws)
	if code == "" {
		code = maestroAgentCode
	}
	return "chat:" + ws.ID + ":" + code
}

func agentOrLoopChatTarget(ws domain.Workspace, code string) string {
	if isChatAttentionCode(code) {
		return "chat:" + ws.ID + ":" + code
	}
	return loopChatTarget(ws)
}

func quietAge(activityMS int64, now time.Time) string {
	if activityMS <= 0 {
		return ""
	}
	at := time.UnixMilli(activityMS)
	d := now.Sub(at)
	if d < briefQuietAfter {
		return ""
	}
	return "quiet for " + formatAge(d)
}

func formatAge(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	h := int(d.Hours())
	if h < 48 {
		return fmt.Sprintf("%dh", h)
	}
	days := h / 24
	return fmt.Sprintf("%dd", days)
}

func sortBriefs(briefs []domain.WorkspaceBrief) {
	rank := func(b domain.WorkspaceBrief) int {
		switch b.Bucket {
		case domain.BriefBucketNeeds:
			return 0
		case domain.BriefBucketWorking:
			return 1
		default:
			return 2
		}
	}
	needsRank := func(b domain.WorkspaceBrief) int {
		if len(b.Reasons) == 0 {
			return 99
		}
		switch b.Reasons[0] {
		case domain.BriefReasonPermission, domain.BriefReasonQuestion, domain.BriefReasonEscalation,
			domain.BriefReasonRetryableFailure, domain.BriefReasonFailure, domain.BriefReasonPlanApproval,
			domain.BriefReasonFindings:
			return 0
		case domain.BriefReasonPRAttention:
			return 1
		case domain.BriefReasonUnseenChat:
			return 2
		}
		return 3
	}
	queued := func(b domain.WorkspaceBrief) bool {
		return b.State == string(domain.StateQueued)
	}
	for i := 0; i < len(briefs); i++ {
		for j := i + 1; j < len(briefs); j++ {
			a, b := briefs[i], briefs[j]
			swap := false
			if rank(b) < rank(a) {
				swap = true
			} else if rank(b) == rank(a) {
				switch a.Bucket {
				case domain.BriefBucketNeeds:
					if b.LastActivityAt > a.LastActivityAt {
						swap = true
					} else if b.LastActivityAt == a.LastActivityAt && needsRank(b) < needsRank(a) {
						swap = true
					} else if b.LastActivityAt == a.LastActivityAt && needsRank(b) == needsRank(a) && b.WorkspaceID < a.WorkspaceID {
						swap = true
					}
				case domain.BriefBucketWorking:
					if queued(a) && !queued(b) {
						swap = true
					} else if queued(a) == queued(b) && b.LastActivityAt > a.LastActivityAt {
						swap = true
					} else if queued(a) == queued(b) && b.LastActivityAt == a.LastActivityAt && b.WorkspaceID < a.WorkspaceID {
						swap = true
					}
				default:
					if b.ChangedSinceViewed && !a.ChangedSinceViewed {
						swap = true
					} else if b.ChangedSinceViewed == a.ChangedSinceViewed {
						if b.LastViewedAt > a.LastViewedAt {
							swap = true
						} else if b.LastViewedAt == a.LastViewedAt && b.LastActivityAt > a.LastActivityAt {
							swap = true
						} else if b.LastViewedAt == a.LastViewedAt && b.LastActivityAt == a.LastActivityAt && b.WorkspaceID < a.WorkspaceID {
							swap = true
						}
					}
				}
			}
			if swap {
				briefs[i], briefs[j] = briefs[j], briefs[i]
			}
		}
	}
}
