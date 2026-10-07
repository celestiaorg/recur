package engine

import (
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

func TestBuildWorkspaceBriefReasonPrecedence(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)
	base := domain.Workspace{
		ID: "W1", ProjectID: "p", Short: "Auth", Branch: "recur/auth",
		State: domain.StatePaused, Last: "waiting", LastActivityAt: frozen.UnixMilli(),
	}
	perm := &domain.PermissionRequest{ID: "perm-1", WorkspaceID: "W1", AgentCode: "PA", Tool: "Bash"}
	q := &domain.UserQuestion{ID: "q-1", WorkspaceID: "W1", AgentCode: "PA", Question: "Which auth library?"}
	esc := &domain.Escalation{ID: "e-1", WorkspaceID: "W1", Question: "Ship with caveats?"}
	unapproved := &domain.Plan{Summary: "Add auth", Body: "Touch login.go", Approved: false, Steps: []domain.PlanStep{
		{ID: "S1", Title: "Add login", Status: domain.StepPending},
	}}

	tests := []struct {
		name   string
		in     briefInputs
		want   string
		bucket string
	}{
		{
			name: "permission beats question",
			in: briefInputs{
				ws: base, perm: perm, question: q, now: frozen,
			},
			want: domain.BriefReasonPermission, bucket: domain.BriefBucketNeeds,
		},
		{
			name: "question beats escalation",
			in: briefInputs{
				ws: base, question: q, escalation: esc, now: frozen,
			},
			want: domain.BriefReasonQuestion, bucket: domain.BriefBucketNeeds,
		},
		{
			name: "retryable failure",
			in: briefInputs{
				ws:  domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateFailed, Retryable: true, LastError: "rate limit"},
				now: frozen,
			},
			want: domain.BriefReasonRetryableFailure, bucket: domain.BriefBucketNeeds,
		},
		{
			name: "plan approval",
			in: briefInputs{
				ws:  domain.Workspace{ID: "W1", Short: "Auth", Plan: unapproved},
				now: frozen,
			},
			want: domain.BriefReasonPlanApproval, bucket: domain.BriefBucketNeeds,
		},
		{
			name: "open findings",
			in: briefInputs{
				ws: domain.Workspace{ID: "W1", Short: "Auth"}, openFindings: 2, now: frozen,
			},
			want: domain.BriefReasonFindings, bucket: domain.BriefBucketNeeds,
		},
		{
			name: "active agent beats leftover findings",
			in: briefInputs{
				ws:           domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateCoding, Phase: "editing login.go"},
				agents:       []domain.Agent{{Code: "CO", Name: "Coder", Status: "working", Task: "editing login.go"}},
				openFindings: 3, now: frozen,
			},
			want: domain.BriefReasonActiveAgent, bucket: domain.BriefBucketWorking,
		},
		{
			name: "unfinished plan is pickup",
			in: briefInputs{
				ws: domain.Workspace{
					ID: "W1", Short: "Auth",
					Plan: &domain.Plan{Approved: true, Summary: "Auth", Body: "Go", Steps: []domain.PlanStep{
						{ID: "S1", Title: "Add login", Status: domain.StepDone},
						{ID: "S2", Title: "Add tests", Status: domain.StepPending},
					}},
				},
				now: frozen,
			},
			want: domain.BriefReasonUnfinishedPlan, bucket: domain.BriefBucketPickup,
		},
		{
			name: "dirty idle work",
			in: briefInputs{
				ws:  domain.Workspace{ID: "W1", Short: "Auth", Diff: domain.DiffStat{Add: 4, Del: 1}},
				now: frozen,
			},
			want: domain.BriefReasonDirtyIdle, bucket: domain.BriefBucketPickup,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok := buildWorkspaceBrief(tt.in)
			if !ok {
				t.Fatalf("buildWorkspaceBrief() ok=false, want brief")
			}
			if len(got.Reasons) == 0 || got.Reasons[0] != tt.want {
				t.Errorf("buildWorkspaceBrief() reasons[0] = %v, want %s", got.Reasons, tt.want)
			}
			if got.Bucket != tt.bucket {
				t.Errorf("buildWorkspaceBrief() bucket = %q, want %q", got.Bucket, tt.bucket)
			}
			if strings.TrimSpace(got.Now) == "" || strings.TrimSpace(got.Next) == "" {
				t.Errorf("buildWorkspaceBrief() now/next empty: %q / %q", got.Now, got.Next)
			}
			if got.Primary.Target == "" {
				t.Error("buildWorkspaceBrief() missing primary target")
			}
		})
	}
}

func TestBuildWorkspaceBriefChatNowNamesTheAction(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)
	named := []domain.Session{{
		UpdatedAt:  10,
		ConfigJSON: `{"code":"PA","title":"oauth weirdness"}`,
	}}

	t.Run("recent chat quotes the tab and says continue", func(t *testing.T) {
		got, ok := buildWorkspaceBrief(briefInputs{
			ws: domain.Workspace{ID: "W1", Short: "Auth"}, headers: named, now: frozen,
		})
		if !ok {
			t.Fatal("buildWorkspaceBrief() ok=false, want brief")
		}
		if got.Reasons[0] != domain.BriefReasonRecentChat {
			t.Fatalf("reasons[0] = %v, want recent_chat", got.Reasons)
		}
		if got.Now != "Continue the “oauth weirdness” chat" {
			t.Errorf("now = %q, want continue + quoted tab", got.Now)
		}
	})

	t.Run("generic New chat title stays canned", func(t *testing.T) {
		got, ok := buildWorkspaceBrief(briefInputs{
			ws:      domain.Workspace{ID: "W1", Short: "Auth"},
			headers: []domain.Session{{UpdatedAt: 10, ConfigJSON: `{"code":"PA","title":"New chat"}`}},
			now:     frozen,
		})
		if !ok {
			t.Fatal("buildWorkspaceBrief() ok=false, want brief")
		}
		if got.Now != "Continue the latest chat" {
			t.Errorf("now = %q, want canned continue", got.Now)
		}
	})

	t.Run("unseen reply names the chat", func(t *testing.T) {
		got, ok := buildWorkspaceBrief(briefInputs{
			ws:      domain.Workspace{ID: "W1", Short: "Auth"},
			agents:  []domain.Agent{{Code: "PA", Unseen: true, Name: "oauth weirdness"}},
			headers: named, now: frozen,
		})
		if !ok {
			t.Fatal("buildWorkspaceBrief() ok=false, want brief")
		}
		if got.Reasons[0] != domain.BriefReasonUnseenChat {
			t.Fatalf("reasons[0] = %v, want unseen_chat", got.Reasons)
		}
		if got.Now != "New reply in “oauth weirdness”" {
			t.Errorf("now = %q, want new-reply + quoted tab", got.Now)
		}
	})

	t.Run("parked question names the chat", func(t *testing.T) {
		got, ok := buildWorkspaceBrief(briefInputs{
			ws: domain.Workspace{ID: "W1", Short: "Auth"},
			question: &domain.UserQuestion{
				ID: "q-1", WorkspaceID: "W1", AgentCode: "PA", Question: "Which auth library?",
			},
			headers: named, now: frozen,
		})
		if !ok {
			t.Fatal("buildWorkspaceBrief() ok=false, want brief")
		}
		if got.Now != "“oauth weirdness” is waiting for an answer" {
			t.Errorf("now = %q, want quoted chat waiting", got.Now)
		}
	})
}

func TestBuildWorkspaceBriefSkipsArchivedAndSettled(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)
	_, ok := buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "gone", Archived: true, Short: "x"}, now: frozen,
	})
	if ok {
		t.Error("archived workspace must not appear")
	}
	_, ok = buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "done", State: domain.StateMerged, Short: "shipped"}, now: frozen,
	})
	if ok {
		t.Error("merged workspace without attention must not appear")
	}
	_, ok = buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "main", IsMain: true, Short: "repo"}, now: frozen,
	})
	if ok {
		t.Error("idle main working copy must not appear")
	}
}

func TestBuildWorkspaceBriefPlanRoutesToPlanPanel(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)
	got, ok := buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "W1", Short: "Auth", Plan: &domain.Plan{
			Summary: "Add auth", Body: "Touch login.go", Approved: false,
			Steps: []domain.PlanStep{{ID: "S1", Title: "Add login", Status: domain.StepPending}},
		}},
		now: frozen,
	})
	if !ok {
		t.Fatal("plan approval ok=false")
	}
	if got.Primary.Target != "workspace:W1:plan" {
		t.Errorf("plan approval target = %q, want workspace:W1:plan", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "W1", Short: "Auth", Plan: &domain.Plan{
			Approved: true, Summary: "Auth", Body: "Go",
			Steps: []domain.PlanStep{
				{ID: "S1", Title: "Add login", Status: domain.StepDone},
				{ID: "S2", Title: "Add tests", Status: domain.StepPending},
			},
		}},
		now: frozen,
	})
	if !ok {
		t.Fatal("unfinished plan ok=false")
	}
	if got.Primary.Target != "workspace:W1:plan" {
		t.Errorf("unfinished plan target = %q, want workspace:W1:plan", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "W1", Short: "Auth"}, openFindings: 2, now: frozen,
	})
	if !ok {
		t.Fatal("findings ok=false")
	}
	if got.Primary.Target != "workspace:W1:findings" {
		t.Errorf("findings target = %q, want workspace:W1:findings", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "W1", Short: "Auth", Diff: domain.DiffStat{Add: 4, Del: 1}}, now: frozen,
	})
	if !ok {
		t.Fatal("dirty idle ok=false")
	}
	if got.Primary.Target != "workspace:W1:review" {
		t.Errorf("dirty idle target = %q, want workspace:W1:review", got.Primary.Target)
	}
}

func TestBuildWorkspaceBriefLoopStatusOpensChat(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)

	got, ok := buildWorkspaceBrief(briefInputs{
		ws:  domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateFailed, Retryable: true, RunStarted: true, LoopChatCode: "PA"},
		now: frozen,
	})
	if !ok {
		t.Fatal("retryable failure ok=false")
	}
	if got.Primary.Target != "chat:W1:PA" {
		t.Errorf("retryable target = %q, want chat:W1:PA", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws:  domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateFailed, LastError: "boom", RunStarted: true},
		now: frozen,
	})
	if !ok {
		t.Fatal("failure ok=false")
	}
	if got.Primary.Target != "chat:W1:MA" {
		t.Errorf("failure target = %q, want chat:W1:MA", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws:     domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateCoding, RunStarted: true, LoopChatCode: "PA"},
		agents: []domain.Agent{{Code: "CO", Name: "Coder", Status: "working", Task: "editing login.go"}},
		now:    frozen,
	})
	if !ok {
		t.Fatal("loop specialist ok=false")
	}
	if got.Primary.Target != "chat:W1:PA" {
		t.Errorf("active specialist target = %q, want chat:W1:PA", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws:     domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateCoding},
		agents: []domain.Agent{{Code: "PA", Name: "Chat", Status: "working", Task: "writing tests"}},
		now:    frozen,
	})
	if !ok {
		t.Fatal("pair chat ok=false")
	}
	if got.Primary.Target != "chat:W1:PA" {
		t.Errorf("active pair chat target = %q, want chat:W1:PA", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws:         domain.Workspace{ID: "W1", Short: "Auth", State: domain.StateEscalated, RunStarted: true, LoopChatCode: "PA"},
		escalation: &domain.Escalation{ID: "e-1", WorkspaceID: "W1", Question: "Ship with caveats?"},
		now:        frozen,
	})
	if !ok {
		t.Fatal("escalation ok=false")
	}
	if got.Primary.Target != "chat:W1:PA" {
		t.Errorf("escalation target = %q, want chat:W1:PA", got.Primary.Target)
	}

	got, ok = buildWorkspaceBrief(briefInputs{
		ws:  domain.Workspace{ID: "W1", Short: "Auth", State: domain.StatePaused, Diff: domain.DiffStat{Add: 2, Del: 0}},
		now: frozen,
	})
	if !ok {
		t.Fatal("idle paused+dirty ok=false")
	}
	if got.Reasons[0] != domain.BriefReasonDirtyIdle {
		t.Errorf("idle paused reason = %v, want dirty_idle", got.Reasons)
	}
}

func TestBuildWorkspaceBriefPRHost(t *testing.T) {
	frozen := time.Date(2026, 8, 31, 12, 0, 0, 0, time.UTC)
	pr := &domain.PR{ID: "pr-1", Number: 12, Title: "Fix checkout", State: "In review", ReviewRequested: true}
	got, ok := buildWorkspaceBrief(briefInputs{
		ws: domain.Workspace{ID: "host", HostPRID: "pr-1", Short: "hidden"},
		pr: pr, now: frozen,
	})
	if !ok {
		t.Fatal("PR host with review attention must appear")
	}
	if !got.IsPRHost || got.PRID != "pr-1" || got.PRNumber != 12 {
		t.Errorf("PR host identity = %+v", got)
	}
	if got.Reasons[0] != domain.BriefReasonPRAttention {
		t.Errorf("reasons[0] = %s, want pr_attention", got.Reasons[0])
	}
	if got.Primary.Target != "pr:pr-1" {
		t.Errorf("primary target = %q, want pr:pr-1", got.Primary.Target)
	}
}

func TestSortBriefsStable(t *testing.T) {
	briefs := []domain.WorkspaceBrief{
		{WorkspaceID: "pickup-old", Bucket: domain.BriefBucketPickup, LastActivityAt: 10, LastViewedAt: 1},
		{WorkspaceID: "needs-unseen", Bucket: domain.BriefBucketNeeds, Reasons: []string{domain.BriefReasonUnseenChat}, LastActivityAt: 50},
		{WorkspaceID: "needs-perm", Bucket: domain.BriefBucketNeeds, Reasons: []string{domain.BriefReasonPermission}, LastActivityAt: 20},
		{WorkspaceID: "work-active", Bucket: domain.BriefBucketWorking, State: string(domain.StateCoding), LastActivityAt: 40},
		{WorkspaceID: "work-queued", Bucket: domain.BriefBucketWorking, State: string(domain.StateQueued), LastActivityAt: 90},
		{WorkspaceID: "pickup-new", Bucket: domain.BriefBucketPickup, ChangedSinceViewed: true, LastActivityAt: 5, LastViewedAt: 2},
	}
	sortBriefs(briefs)
	want := []string{"needs-unseen", "needs-perm", "work-active", "work-queued", "pickup-new", "pickup-old"}
	for i, id := range want {
		if briefs[i].WorkspaceID != id {
			t.Errorf("sortBriefs()[%d] = %s, want %s (got %v)", i, briefs[i].WorkspaceID, id, idsOf(briefs))
			break
		}
	}
}

func TestSortBriefsNeedsNewestFirst(t *testing.T) {
	briefs := []domain.WorkspaceBrief{
		{WorkspaceID: "old-pr", Bucket: domain.BriefBucketNeeds, Reasons: []string{domain.BriefReasonPRAttention}, LastActivityAt: 10},
		{WorkspaceID: "just-now", Bucket: domain.BriefBucketNeeds, Reasons: []string{domain.BriefReasonUnseenChat}, LastActivityAt: 90},
		{WorkspaceID: "mid-findings", Bucket: domain.BriefBucketNeeds, Reasons: []string{domain.BriefReasonFindings}, LastActivityAt: 50},
	}
	sortBriefs(briefs)
	want := []string{"just-now", "mid-findings", "old-pr"}
	for i, id := range want {
		if briefs[i].WorkspaceID != id {
			t.Errorf("sortBriefs()[%d] = %s, want %s (got %v)", i, briefs[i].WorkspaceID, id, idsOf(briefs))
			break
		}
	}
}

func idsOf(briefs []domain.WorkspaceBrief) []string {
	out := make([]string, len(briefs))
	for i, b := range briefs {
		out[i] = b.WorkspaceID
	}
	return out
}

func TestEffectiveWorkspaceActivityLegacyFallback(t *testing.T) {
	ws := domain.Workspace{ID: "legacy"}
	headers := []domain.Session{{UpdatedAt: 40}, {UpdatedAt: 10}}
	pr := &domain.PR{UpdatedAt: "2026-08-31T12:00:00Z"}
	got := effectiveWorkspaceActivity(ws, headers, 25, pr)
	if got < 40 {
		t.Errorf("effectiveWorkspaceActivity() = %d, want at least session 40", got)
	}
	ws.LastActivityAt = 99
	if got := effectiveWorkspaceActivity(ws, headers, 25, pr); got != 99 {
		t.Errorf("persisted LastActivityAt must win: got %d want 99", got)
	}
}

func TestMarkWorkspaceViewedWatermark(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	frozen := time.Date(2026, 8, 31, 15, 0, 0, 0, time.UTC)
	eng.nowFn = func() time.Time { return frozen }
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "W-view", Short: "v", LastActivityAt: 200, SeenActivityAt: 50,
	}); err != nil {
		t.Fatal(err)
	}
	eng.MarkWorkspaceViewed("W-view", 180)
	got, ok := st.Workspace("W-view")
	if !ok {
		t.Fatal("workspace missing")
	}
	if got.SeenActivityAt != 180 {
		t.Errorf("SeenActivityAt = %d, want 180", got.SeenActivityAt)
	}
	if got.LastViewedAt != frozen.UnixMilli() {
		t.Errorf("LastViewedAt = %d, want %d", got.LastViewedAt, frozen.UnixMilli())
	}
	eng.MarkWorkspaceViewed("W-view", 100) // older must not rewind
	got, _ = st.Workspace("W-view")
	if got.SeenActivityAt != 180 {
		t.Errorf("SeenActivityAt rewound to %d", got.SeenActivityAt)
	}
	if !sink.has("briefing.updated") {
		t.Error("MarkWorkspaceViewed must emit briefing.updated")
	}
}

func TestChangedSinceViewedNeedsAPriorView(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	if _, err := st.AddProject(domain.Project{ID: "p1", Name: "P"}); err != nil {
		t.Fatal(err)
	}
	// Never viewed (SeenActivityAt zero): activity alone must not raise the
	// "Since you were here" cue — the user was never here.
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "W-fresh", ProjectID: "p1", Short: "f", State: domain.StatePaused,
		LastActivityAt: 100, Last: "created",
	}); err != nil {
		t.Fatal(err)
	}
	// Viewed at 100, then more activity: cue fires.
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "W-back", ProjectID: "p1", Short: "b", State: domain.StatePaused,
		LastActivityAt: 200, SeenActivityAt: 100, Last: "Coder pushed a fix",
	}); err != nil {
		t.Fatal(err)
	}
	snap := eng.BriefingSnapshot()
	byID := map[string]domain.WorkspaceBrief{}
	for _, b := range snap.Workspaces {
		byID[b.WorkspaceID] = b
	}
	if b, ok := byID["W-fresh"]; !ok || b.ChangedSinceViewed {
		t.Errorf("never-viewed workspace must not be ChangedSinceViewed: %+v ok=%v", b, ok)
	}
	if b, ok := byID["W-back"]; !ok || !b.ChangedSinceViewed {
		t.Errorf("viewed-then-active workspace must be ChangedSinceViewed: %+v ok=%v", b, ok)
	}
}

func TestTouchWorkspaceActivityMonotonic(t *testing.T) {
	eng, st, sink, _ := newTestEngine(t, nil)
	if _, err := st.AddWorkspace(domain.Workspace{ID: "W-act", Short: "a", LastActivityAt: 50, Last: "old"}); err != nil {
		t.Fatal(err)
	}
	eng.touchWorkspaceActivity("W-act", "newer", time.UnixMilli(40)) // older ts
	got, _ := st.Workspace("W-act")
	if got.LastActivityAt != 50 {
		t.Errorf("LastActivityAt moved backward: %d", got.LastActivityAt)
	}
	eng.touchWorkspaceActivity("W-act", "newer", time.UnixMilli(80))
	got, _ = st.Workspace("W-act")
	if got.LastActivityAt != 80 || got.Last != "newer" {
		t.Errorf("touchWorkspaceActivity() last=%q at=%d, want newer/80", got.Last, got.LastActivityAt)
	}
	if !sink.has("briefing.updated") {
		t.Error("touchWorkspaceActivity must emit briefing.updated")
	}
}

func TestBriefingSnapshotAndContext(t *testing.T) {
	eng, st, _, _ := newTestEngine(t, nil)
	frozen := time.Date(2026, 8, 31, 16, 0, 0, 0, time.UTC)
	eng.nowFn = func() time.Time { return frozen }
	if _, err := st.AddProject(domain.Project{ID: "p1", Name: "Storefront"}); err != nil {
		t.Fatal(err)
	}
	plan := &domain.Plan{
		Summary: "Fix checkout", Body: "Touch Cart.svelte", Approved: true,
		Steps: []domain.PlanStep{
			{ID: "S1", Title: "Fix the flake", Status: domain.StepDone},
			{ID: "S2", Title: "Add a retry", Status: domain.StepPending},
		},
	}
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "W-live", ProjectID: "p1", Short: "Checkout flake", State: domain.StateCoding,
		Phase: "editing tests", Plan: plan, LastActivityAt: frozen.UnixMilli(),
	}); err != nil {
		t.Fatal(err)
	}
	st.UpsertAgent("W-live", domain.Agent{WorkspaceID: "W-live", Code: "CO", Name: "Coder", Status: "working", Task: "editing tests"})
	if _, err := st.AddWorkspace(domain.Workspace{
		ID: "W-arch", ProjectID: "p1", Archived: true, Short: "gone", Plan: plan,
	}); err != nil {
		t.Fatal(err)
	}

	snap := eng.BriefingSnapshot()
	if snap.Counts.Working != 1 {
		t.Errorf("BriefingSnapshot() working = %d, want 1 (got %+v)", snap.Counts.Working, snap)
	}
	if len(snap.Workspaces) != 1 || snap.Workspaces[0].WorkspaceID != "W-live" {
		t.Fatalf("BriefingSnapshot() workspaces = %+v, want W-live only", snap.Workspaces)
	}
	b := snap.Workspaces[0]
	if b.ProjectName != "Storefront" || b.Reasons[0] != domain.BriefReasonActiveAgent {
		t.Errorf("brief = %+v", b)
	}

	ctx, err := eng.WorkspaceContext("W-live")
	if err != nil {
		t.Fatalf("WorkspaceContext: %v", err)
	}
	if len(ctx.PlanSteps) != 2 {
		t.Errorf("WorkspaceContext plan steps = %d, want 2", len(ctx.PlanSteps))
	}
	if len(ctx.Agents) != 1 || ctx.Agents[0].Code != "CO" {
		t.Errorf("WorkspaceContext agents = %+v", ctx.Agents)
	}
	if _, err := eng.WorkspaceContext("nope"); err == nil {
		t.Error("WorkspaceContext(missing) must error")
	}
}
