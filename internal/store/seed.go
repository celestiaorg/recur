package store

import "recur/internal/domain"

// CSS color vars.
const (
	cAmber   = "var(--amber)"
	cCyan    = "var(--cyan)"
	cEmerald = "var(--emerald)"
	cCoral   = "var(--coral)"
	cIndigo  = "var(--indigo)"
	cPRpurp  = "#b08bd8"
)

// h builds a DiffLine.
func h(old, new, typ, code string) domain.DiffLine {
	return domain.DiffLine{Old: old, New: new, Type: typ, CodeHTML: code}
}

func hx(old, new, typ, code, extra string) domain.DiffLine {
	return domain.DiffLine{Old: old, New: new, Type: typ, CodeHTML: code, ExtraHTML: extra}
}

func ds(a, d int) domain.DiffStat { return domain.DiffStat{Add: a, Del: d} }

// seed loads BOTH the default config and the demo dataset. Used by NewInMemory
// (and tests) where the full mock world is wanted. The on-disk app path
// (store.open) calls seedDefaults only — never seedDemo.
func (s *Store) seed() {
	s.seedDefaults()
	s.seedDemo()
}

// seedDefaults sets the config a brand-new app needs regardless of demo data:
// the global policy (autonomy only — rails retired; [[policy]] is the guard),
// agent/provider settings + caps, and a ZEROED resource so a clean app's status
// bar reads 0 loops / 0 needs-you / 0 iters.
func (s *Store) seedDefaults() {
	s.policy = domain.Policy{
		Autonomy: domain.AutonomyBalanced,
	}
	// Per-mode models are fully-qualified ModelRefs (`provider/id`) — the
	// provider prefix routes each mode's turns. They default to the pinned
	// Sonnet 5.5 id; effort defaults to high (Claude Code's balanced default). Editable in Settings → Models (one row per mode).
	highFlags := func() map[string]string {
		return map[string]string{"effort": "high"}
	}
	s.settings = domain.Settings{
		ModelAsk:                   "claude-code/claude-sonnet-5-5",
		ModelPlan:                  "claude-code/claude-sonnet-5-5",
		ModelAgent:                 "claude-code/claude-sonnet-5-5",
		ModelReview:                "claude-code/claude-sonnet-5-5",
		ModelLoop:                  "claude-code/claude-sonnet-5-5",
		ModelWorkspaceTitle:        "claude-code/claude-haiku-4-5-20251001",
		FlagsAsk:                   highFlags(),
		FlagsPlan:                  highFlags(),
		FlagsAgent:                 highFlags(),
		FlagsReview:                highFlags(),
		FlagsLoop:                  highFlags(),
		FlagsWorkspaceTitle:        map[string]string{"effort": "low"},
		BypassPermissions:          false,
		MaxLoops:                   4,
		MaxAgents:                  8,
		DefaultMergeMethod:         domain.MergeMethodMerge,
		DefaultUpdateBaseMethod:    domain.UpdateBaseMerge,
		DefaultSquashCommitMessage: domain.SquashCommitTitleAndCommits,
		DefaultMergeCommitMessage:  domain.MergeCommitTitle,
	}
	s.resource = domain.Resource{} // clean: 0 loops / 0 needs-you / 0 iters
}

// seedDemo loads the mock dataset (projects, loops,
// agents, feed, PRs, escalation, inbox) plus the demo status-bar numbers.
// Used by NewInMemory, persistence tests, and cmd/seeddump (e2e fixtures).
func (s *Store) seedDemo() {
	s.projects = []domain.Project{
		{ID: "storefront", Name: "storefront", Pic: "S", Color: cCyan, Open: true, RepoPath: "~/code/storefront", BaseBranch: "main"},
		{ID: "payments", Name: "payments-api", Pic: "P", Color: cAmber, Open: true, RepoPath: "~/code/payments-api", BaseBranch: "main"},
		{ID: "marketing", Name: "marketing-site", Pic: "M", Color: cIndigo, Open: false, Idle: true, RepoPath: "~/code/marketing-site", BaseBranch: "main"},
	}

	s.workspaces = []domain.Workspace{
		{ID: "LOOP-2218", ProjectID: "payments", Group: domain.GroupNeeds, Goal: "Rate-limit the public API", Short: "Rate-limit the public API", State: domain.StateEscalated, StatusLabel: "Needs you", Phase: "asking a question", Iteration: 6, Progress: .85, Autonomy: domain.AutonomyBalanced, Branch: "recur/rate-limit", Diff: ds(0, 0), AgentCodes: []string{"MA"}, Last: "escalated: ambiguous burst-limit spec", ElapsedLabel: "paused 12m"},
		{ID: "LOOP-2231", ProjectID: "storefront", Group: domain.GroupActive, Goal: "Add Apple & Google sign-in to checkout", Short: "Add Apple & Google sign-in", State: domain.StateVerifying, StatusLabel: "Verifying", Phase: "browsing the app", Iteration: 3, Progress: .72, Autonomy: domain.AutonomyBalanced, Branch: "recur/oauth-checkout", Diff: ds(218, 12), AgentCodes: []string{"MA", "CO", "QA", "RV"}, Last: "QA: clicking “Sign in with Apple”…", ElapsedLabel: "14m 09s",
			Convergence: []domain.ConvPoint{{Iteration: 1, Findings: 12, Color: cCoral}, {Iteration: 2, Findings: 4, Color: cAmber}, {Iteration: 3, Findings: 0, Color: cEmerald}}},
		{ID: "LOOP-2229", ProjectID: "storefront", Group: domain.GroupActive, Goal: "Migrate analytics to Postgres 16", Short: "Migrate analytics to PG16", State: domain.StateCoding, StatusLabel: "Coding", Phase: "iteration 2", Iteration: 2, Progress: .45, Autonomy: domain.AutonomyBalanced, Branch: "recur/pg16", Diff: ds(540, 110), AgentCodes: []string{"MA", "CO"}, Last: "Coder: rewriting migration 0042…", ElapsedLabel: "31m"},
		{ID: "LOOP-2235", ProjectID: "storefront", Group: domain.GroupActive, Goal: "Dark mode for the dashboard", Short: "Dark mode for dashboard", State: domain.StateReviewing, StatusLabel: "Reviewing", Phase: "iteration 1", Iteration: 1, Progress: .3, Autonomy: domain.AutonomyBalanced, Branch: "recur/dark-mode", Diff: ds(96, 8), AgentCodes: []string{"MA", "CO", "RV"}, Last: "Reviewer: 7 findings on contrast…", ElapsedLabel: "8m"},
		{ID: "LOOP-2224", ProjectID: "storefront", Group: domain.GroupReady, Goal: "Fix flaky checkout E2E tests", Short: "Fix flaky E2E tests", State: domain.StatePROpen, StatusLabel: "Ready", Phase: "PR #479 open", Iteration: 5, Progress: 1, Autonomy: domain.AutonomyBalanced, Branch: "recur/fix-e2e", Diff: ds(34, 60), AgentCodes: []string{"MA", "TE"}, Last: "converged · awaiting your review", ElapsedLabel: "1h 02m"},
	}

	s.seedAgents()
	s.seedFeed()
	s.seedPRs()
	s.seedEscalation()
	s.seedInbox()

	// Demo status-bar numbers (the clean default is zeroed in seedDefaults).
	s.resource = domain.Resource{LoopsRunning: 3, NeedsYou: 1, ItersToday: 37}
}

func (s *Store) seedAgents() {
	s.agents["LOOP-2231"] = []domain.Agent{
		{ID: "LOOP-2231-MA", WorkspaceID: "LOOP-2231", Code: "MA", Name: "Maestro", Role: "orchestrator · plans & delegates", Model: "opus", Status: "directing", Color: cAmber, Task: "Coordinating iteration 3 → verification → PR.", Cur: -1, Steps: []domain.AgentStep{
			{Kind: "plan", Text: "Decomposed task into config PR + UI PR"},
			{Kind: "spawn", Text: "Created Reviewer, Coder, Tester, QA on demand"},
			{Kind: "watch", Text: "Monitoring QA verification, then will open PRs"},
		}},
		{ID: "LOOP-2231-CO", WorkspaceID: "LOOP-2231", Code: "CO", Name: "Coder", Role: "writes & edits code", Model: "sonnet", Status: "idle", Color: cCyan, Task: "Last: resolve reviewer finding on token refresh.", Cur: -1, Steps: []domain.AgentStep{
			{Kind: "read", Text: "src/auth/oauth.ts"},
			{Kind: "read", Text: "src/hooks/useOAuthSession.ts"},
			{Kind: "edit", Text: "src/hooks/useOAuthSession.ts", Diff: &domain.DiffStat{Add: 34, Del: 12}},
			{Kind: "run", Text: "pnpm test auth → 6 passed", State: "ok"},
			{Kind: "done", Text: "handed back to Maestro"},
		}},
		{ID: "LOOP-2231-QA", WorkspaceID: "LOOP-2231", Code: "QA", Name: "QA / Browser", Role: "drives the live app like a user", Model: "sonnet", Status: "working", Color: cCoral, Task: "Verify Apple & Google sign-in end to end in the browser.", Cur: 4, Steps: []domain.AgentStep{
			{Kind: "run", Text: "pnpm dev --port 5173 → ready", State: "run"},
			{Kind: "open", Text: "navigated to http://localhost:5173/checkout", State: "read"},
			{Kind: "click", Text: "“Sign in with Apple” → Apple consent screen", State: "ok"},
			{Kind: "check", Text: "redirect returned a valid session ✓", State: "ok"},
			{Kind: "click", Text: "“Sign in with Google” …", State: "cur"},
		}},
		{ID: "LOOP-2231-RV", WorkspaceID: "LOOP-2231", Code: "RV", Name: "Reviewer", Role: "critiques diffs, files findings", Model: "opus", Status: "idle", Color: cIndigo, Task: "Last: re-review after iteration 3.", Cur: -1, Steps: []domain.AgentStep{
			{Kind: "read", Text: "diff for #482 (+218 −12)"},
			{Kind: "note", Text: "iter 1: 12 findings (secrets, error handling)"},
			{Kind: "note", Text: "iter 3: 0 findings"},
			{Kind: "done", Text: "approved · re-requested human review"},
		}},
	}
}

func (s *Store) seedFeed() {
	// Kind is stamped to match each row's classification (the frontend reads it
	// directly now that the OUTCOME_RE heuristic is gone).
	s.feed["LOOP-2231"] = []domain.FeedItem{
		{ID: "f1", AgentCode: "MA", Name: "Maestro", Kind: domain.FeedKindOutcome, Text: "Review converged at 0 findings — advancing to live verification before opening PR #2.", Time: "45s"},
		{ID: "f2", AgentCode: "QA", Name: "QA / Browser", Kind: domain.FeedKindOutcome, Text: `Booted dev server on <code>:5173</code>, navigating to <code>/checkout</code> to test the Apple flow.`, Time: "38s", ClickAgent: "QA"},
		{ID: "f3", AgentCode: "QA", Name: "QA / Browser", Kind: domain.FeedKindOutcome, Text: `Clicked “Sign in with Apple” → redirect OK → returned with valid session. <span style="color:var(--emerald)">✓ pass</span>`, Time: "12s", ClickAgent: "QA"},
		{ID: "f4", IsDeviation: true, Kind: domain.FeedKindDeviation, Name: "Maestro", Text: "Spawned a second, security-focused reviewer for this iteration.", Rule: "default = one reviewer", Why: `diff touches <code>auth/oauth.ts</code> — created on demand, then retired.`, Time: "1m"},
		{ID: "f5", AgentCode: "CO", Name: "Coder", Kind: domain.FeedKindOutcome, Text: `Resolved reviewer finding: extracted token refresh into <code>useOAuthSession</code>.`, Time: "1m", ClickAgent: "CO"},
		{ID: "f6", AgentCode: "RV", Name: "Reviewer", Kind: domain.FeedKindOutcome, Text: "Re-reviewed diff after iteration 3 — no remaining findings. Approving.", Time: "1m", ClickAgent: "RV"},
		{ID: "f7", IsDeviation: true, Kind: domain.FeedKindDeviation, Name: "Maestro", Text: "Split the task into 2 PRs instead of 1.", Rule: "default = one PR", Why: "mixed secrets-config with UI wiring — separate PRs are easier to review.", Time: "2m"},
		{ID: "f8", AgentCode: "TE", Name: "Tester", Kind: domain.FeedKindNote, Text: "Added 6 tests for Apple + Google providers — all green.", Time: "3m", ClickAgent: "TE"},
	}
}

func (s *Store) seedEscalation() {
	s.escalations["LOOP-2218"] = domain.Escalation{
		ID: "esc-2218", WorkspaceID: "LOOP-2218", Status: "open", AskedAgo: "12m",
		Question: "When a client goes over the rate limit, what should happen?",
		Context:  `The ticket says <i>"limit bursts to protect the API"</i> but doesn't define the over-limit behavior. I've built the limiter middleware and tested the under-limit path — this one decision is all that's blocking the rest. I won't guess on something that changes the API's public contract.`,
		Options: []domain.EscalationOption{
			{Text: `Reject with <code style="font-family:var(--mono)">429</code>`, Tag: "simplest", Desc: "Clear to clients and easy to reason about. Can feel harsh during legitimate spikes."},
			{Text: "Queue & delay", Tag: "smoothest", Desc: "Absorbs short bursts without errors. Adds latency and some memory pressure under load."},
			{Text: "Token bucket with refill", Tag: "recommended", Desc: "Fair, industry-standard, predictable. Slightly more to implement — I can do it in this loop."},
		},
	}
}
