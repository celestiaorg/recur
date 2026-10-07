package store

import "recur/internal/domain"

func (s *Store) seedPRs() {
	pr482 := domain.PR{
		ID: "482", WorkspaceID: "LOOP-2231", ProjectID: "storefront", Group: "you", Number: 482, Source: domain.SourceGitHub,
		URL:    "https://github.com/acme/app/pull/482",
		Branch: "recur/oauth-checkout", Base: "main",
		Title: "Add OAuth provider config + secrets handling",
		State: "In review", Diff: ds(218, 12), FileCount: 3, ChecksOK: true, When: "2m",
		CreatedAt:   "2026-08-28T09:50:00Z",
		UpdatedAt:   "2026-08-28T09:50:00Z",
		AuthorLogin: "loopbot", ViewerLogin: "avery", ViewerCanWrite: true,
		ReviewRequested: true, AttentionReasons: []string{domain.AttentionReviewRequested},
		Participants: []domain.PRParticipant{
			{Login: "loopbot", Name: "loopbot", Code: "LO", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)"},
			{Login: "avery", Name: "Avery Rourke", Code: "AR", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)"},
			{Login: "sam", Name: "sam"},
			{Name: "PR Responder", Code: "PR", AvatarCSS: "background:#b08bd8;color:#0A0C10"},
			{Name: "Coder", Code: "CO", AvatarCSS: "background:var(--cyan);color:#0A0C10"},
		},
		Reviewers: []domain.PRReviewer{
			{Login: "avery", Name: "Avery Rourke", Code: "AR", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)", State: domain.ReviewStatePending},
			{Login: "sam", Name: "sam", State: domain.ReviewStateApproved},
		},
		Summary: []domain.SummaryPara{
			{Kind: "p", HTML: `Adds <b>Apple and Google sign-in</b> to checkout. Provider config and secret handling ship in this PR; the sign-in buttons in the checkout UI follow in <code>#483</code>.`},
			{Kind: "p", HTML: `<b>What changed</b>`},
			{Kind: "bullets", Items: []string{
				`New OAuth provider registry in <code>auth/oauth.ts</code> for Apple + Google.`,
				`Client secrets now resolved from the secrets manager at startup — never read from <code>process.env</code> or logged.`,
				`Session/token refresh extracted into a reusable <code>useOAuthSession</code> hook.`,
			}},
			{Kind: "p", HTML: `<b>Key decision:</b> tokens are stored in an httpOnly cookie rather than localStorage, so they're not reachable from JS. Flagging because it changes how the client reads auth state.`},
		},
		Focus: domain.FocusNote{Title: "Where to focus your review", Body: `The token-refresh race in <code>useOAuthSession.ts</code> (lines 5–10) is the subtle part — two tabs refreshing at once. The Reviewer added a mutex; worth a human look.`},
		Verify: []domain.VerifyStep{
			{Passed: true, HTML: `Booted the app, opened <b>/checkout</b> on a clean session`},
			{Passed: true, HTML: `<b>Sign in with Apple</b> → consent → returned with a valid session`},
			{Passed: true, HTML: `<b>Sign in with Google</b> → consent → returned with a valid session`},
			{Passed: true, HTML: `Reloaded — session persisted; no token in <b>localStorage</b> or logs`},
		},
		Checks: []domain.Check{
			{Name: "test", Detail: "1m 12s", Passed: true, Status: "pass"},
			{Name: "lint", Detail: "8s", Passed: true, Status: "pass"},
			{Name: "typecheck", Detail: "22s", Passed: true, Status: "pass"},
		},
		Comments: []domain.PRComment{
			{ID: "gh-description", Kind: "description", URL: "https://github.com/acme/app/pull/482#issue-482001", Code: "LO", Author: "loopbot", AuthorLogin: "loopbot", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)", Time: "2m", BodyMD: "Adds Apple and Google sign-in to checkout. Provider config and secret handling ship in this PR.", Reactions: []domain.PRReaction{{Content: "hooray", Count: 3}}},
			{ID: "c1", URL: "https://github.com/acme/app/pull/482#issuecomment-1001", Code: "AR", Author: "Avery Rourke", AuthorLogin: "avery", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)", Time: "2m", Verb: "commented", BodyMD: "Left a comment on oauth.ts about the client secret — see Files changed.", BodyHTML: `Left a comment on <code>oauth.ts</code> about the client secret — see Files changed.`, Reactions: []domain.PRReaction{{Content: "+1", Count: 2}, {Content: "heart", Count: 1, Mine: true}}},
			{ID: "c2", URL: "https://github.com/acme/app/pull/482#issuecomment-1002", Code: "MA", Author: "Maestro", AvatarCSS: "background:var(--amber);color:#0A0C10", Time: "2m", Verb: "commented", BodyHTML: `Got the review notes — I'll address the client-secret comment.`},
			{ID: "c3", URL: "https://github.com/acme/app/pull/482#issuecomment-1003", Code: "CO", Author: "Coder", AvatarCSS: "background:var(--cyan);color:#0A0C10", Time: "now", Verb: "commented", BodyHTML: `Pushed <code>a3f91c2</code> — resolved via the secrets manager. Reviewer re-ran clean, re-requested your review.`},
			{ID: "gh-review-99", URL: "https://github.com/acme/app/pull/482#pullrequestreview-99", Code: "AR", Author: "Avery Rourke", AuthorLogin: "avery", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)", Time: "1m", Verb: "commented", BodyMD: "testing again"},
			{ID: "gh-inline-1", ParentID: "gh-review-99", ReviewID: "99", URL: "https://github.com/acme/app/pull/482#discussion_r1", Code: "AR", Author: "Avery Rourke", AuthorLogin: "avery", AvatarCSS: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)", Time: "1m", Verb: "commented", BodyHTML: `<code>src/auth/oauth.ts:11-12</code>`, BodyMD: "hmm", Path: "src/auth/oauth.ts", Line: 12, StartLine: 11, Side: "RIGHT", DiffHunk: "@@ -4,9 +4,9 @@ provider registry\n import { secrets } from './vault'\n const providers = []\n function register() {\n   return providers\n }\n const unused = 1\n const unused2 = 2\n const unused3 = 3\n-const secret = process.env.OAUTH_CLIENT_SECRET\n+const secret = await secrets.get('oauth/client_secret')", ThreadID: "PRRT_seed99", Reactions: []domain.PRReaction{{Content: "eyes", Count: 1}}},
		},
		Commits: pr482Commits(),
		Files:   pr482Files(),
	}

	prList := []domain.PR{
		pr482,
		{ID: "479", WorkspaceID: "LOOP-2224", ProjectID: "storefront", Group: "you", Number: 479, Source: domain.SourceLocal, Title: "Stabilize flaky checkout E2E tests", Branch: "recur/fix-e2e", Base: "main", Diff: ds(34, 60), FileCount: 5, ChecksOK: true, State: "In review", When: "18m", CreatedAt: "2026-08-28T09:34:00Z", UpdatedAt: "2026-08-28T09:34:00Z",
			Participants: []domain.PRParticipant{
				{Name: "Coder", Code: "CO", AvatarCSS: "background:var(--cyan);color:#0A0C10"},
			}},
		{ID: "471", WorkspaceID: "LOOP-2202", ProjectID: "storefront", Group: "merged", Number: 471, Source: domain.SourceGitHub, URL: "https://github.com/acme/app/pull/471", Title: "Add idempotency keys to the orders API", Branch: "recur/idempotency", Base: "main", Diff: ds(156, 9), FileCount: 7, ChecksOK: true, State: "Merged", When: "1d", CreatedAt: "2026-08-27T09:52:00Z", UpdatedAt: "2026-08-27T09:52:00Z"},
	}
	s.prs = prList
	for _, p := range s.prs {
		s.noteLocalPRNumberLocked(p)
	}
}

// pr482Files is the Files-changed diff for #482.
func pr482Files() []domain.FileDiff {
	return []domain.FileDiff{
		{Path: "src/auth/oauth.ts", Status: "m", Diff: ds(96, 8), Lines: []domain.DiffLine{
			h("", "", "hunk", "@@ -1,6 +1,9 @@ provider registry"),
			h("11", "11", "ctx", `<span class="kw">import</span> { secrets } <span class="kw">from</span> <span class="str">'../lib/secrets'</span>`),
			h("12", "", "del", `<span class="kw">const</span> secret = process.env.<span class="nm">OAUTH_CLIENT_SECRET</span>`),
			hx("", "12", "add", `<span class="kw">const</span> secret = <span class="kw">await</span> secrets.<span class="fn">get</span>(<span class="str">'oauth/client_secret'</span>)`,
				`<div class="icmt"><div class="icmt-pad"><div class="cmt" style="padding:0;border:none"><div class="ca" style="background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)">AR</div><div class="cc"><div class="ch"><span class="cn">Avery Rourke</span><span class="ct">2m</span></div><div class="cm">Good — this was reading from <code>process.env</code> before. Confirm the secrets client is initialised before any provider loads?</div><span class="react" style="margin-top:8px"><span style="width:5px;height:5px;border-radius:50%;background:var(--emerald);display:inline-block"></span>Coder: yes — <code>secrets.init()</code> runs in <code>bootstrap.ts</code> before providers register. ✓ resolved</span></div></div></div></div>`),
			h("13", "13", "ctx", `<span class="kw">export const</span> <span class="fn">providers</span> = {`),
			h("", "14", "add", `  apple: <span class="fn">appleProvider</span>(secret),`),
			h("", "15", "add", `  google: <span class="fn">googleProvider</span>(secret),`),
			h("14", "16", "ctx", `}`),
		}},
		{Path: "src/hooks/useOAuthSession.ts", Status: "a", Diff: ds(88, 0), Lines: []domain.DiffLine{
			h("", "", "hunk", "@@ -0,0 +1,12 @@ new file"),
			h("", "1", "add", `<span class="kw">export function</span> <span class="fn">useOAuthSession</span>() {`),
			hx("", "2", "add", `  <span class="kw">const</span> [session, setSession] = <span class="fn">useState</span>&lt;<span class="ty">Session</span> | <span class="kw">null</span>&gt;(<span class="kw">null</span>)`,
				`<div class="flag"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4-4" stroke-linecap="round"/></svg><div><div class="ft">Reviewer flagged this region for human eyes</div><div class="fm">token refresh can race across two tabs — a mutex was added below (lines 5–10). worth confirming the lock releases on error.</div></div></div>`),
			h("", "3", "add", `  <span class="kw">const</span> refreshing = <span class="fn">useRef</span>(<span class="kw">false</span>) <span class="cm">// mutex guard</span>`),
			h("", "4", "add", ``),
			h("", "5", "add", `  <span class="kw">async function</span> <span class="fn">refresh</span>() {`),
			h("", "6", "add", `    <span class="kw">if</span> (refreshing.current) <span class="kw">return</span>`),
			h("", "7", "add", `    refreshing.current = <span class="kw">true</span>`),
			h("", "8", "add", `    <span class="kw">try</span> { setSession(<span class="kw">await</span> <span class="fn">rotateTokens</span>()) }`),
			h("", "9", "add", `    <span class="kw">finally</span> { refreshing.current = <span class="kw">false</span> }`),
			h("", "10", "add", `  }`),
		}},
		{Path: "src/checkout/SignInButtons.tsx", Status: "m", Diff: ds(34, 4), Lines: []domain.DiffLine{
			h("", "", "hunk", "@@ -8,4 +8,12 @@ checkout buttons"),
			h("8", "8", "ctx", `<span class="kw">return</span> (`),
			h("", "9", "add", `  &lt;<span class="ty">AppleButton</span> onClick={() =&gt; <span class="fn">signIn</span>(<span class="str">'apple'</span>)} /&gt;`),
			h("", "10", "add", `  &lt;<span class="ty">GoogleButton</span> onClick={() =&gt; <span class="fn">signIn</span>(<span class="str">'google'</span>)} /&gt;`),
			h("9", "11", "ctx", `)`),
		}},
	}
}

// pr482Commits is the agent-authored commit list for #482 with per-commit diffs.
func pr482Commits() []domain.Commit {
	return []domain.Commit{
		{Hash: "e8d34b1", Message: "Add tests for Apple + Google providers", AuthorRole: "Tester", AgentCode: "TE", Color: cEmerald, IterLabel: "verify pass", Diff: ds(6, 0), When: "4m", Files: []domain.FileDiff{
			{Path: "src/auth/__tests__/oauth.test.ts", Status: "a", Diff: ds(6, 0), Lines: []domain.DiffLine{
				h("", "", "hunk", "@@ -0,0 +1,6 @@ new file"),
				h("", "1", "add", `<span class="fn">test</span>(<span class="str">'apple sign-in returns a session'</span>, <span class="kw">async</span> () =&gt; {`),
				h("", "2", "add", `  <span class="kw">const</span> s = <span class="kw">await</span> <span class="fn">signIn</span>(<span class="str">'apple'</span>)`),
				h("", "3", "add", `  <span class="fn">expect</span>(s.user).<span class="fn">toBeDefined</span>()`),
				h("", "4", "add", `})`),
				h("", "5", "add", `<span class="fn">test</span>(<span class="str">'google sign-in returns a session'</span>, <span class="kw">async</span> () =&gt; { <span class="cm">/* … */</span> })`),
			}},
		}},
		{Hash: "a3f91c2", Message: "Resolve client secret via secrets manager", AuthorRole: "Coder", AgentCode: "CO", Color: cCyan, IterLabel: "iteration 3 · review fix", Diff: ds(10, 6), When: "6m", Files: []domain.FileDiff{
			{Path: "src/auth/oauth.ts", Status: "m", Diff: ds(10, 6), Lines: []domain.DiffLine{
				h("", "", "hunk", "@@ -10,6 +10,9 @@ provider registry"),
				h("10", "10", "ctx", `<span class="kw">import</span> { secrets } <span class="kw">from</span> <span class="str">'../lib/secrets'</span>`),
				h("11", "", "del", `<span class="kw">const</span> secret = process.env.<span class="nm">OAUTH_CLIENT_SECRET</span>`),
				h("", "11", "add", `<span class="kw">const</span> secret = <span class="kw">await</span> secrets.<span class="fn">get</span>(<span class="str">'oauth/client_secret'</span>)`),
				h("12", "12", "ctx", `<span class="kw">export const</span> <span class="fn">providers</span> = {`),
			}},
		}},
		{Hash: "4b7f0d2", Message: "Harden error handling on token exchange", AuthorRole: "Coder", AgentCode: "CO", Color: cCyan, IterLabel: "iteration 2 · review fix", Diff: ds(28, 6), When: "14m", Files: []domain.FileDiff{
			{Path: "src/auth/oauth.ts", Status: "m", Diff: ds(28, 6), Lines: []domain.DiffLine{
				h("", "", "hunk", "@@ -22,4 +22,12 @@ token exchange"),
				h("22", "22", "ctx", `<span class="kw">async function</span> <span class="fn">exchange</span>(provider, code) {`),
				h("23", "", "del", `  <span class="kw">const</span> res = <span class="kw">await</span> <span class="fn">fetch</span>(tokenUrl, { method: <span class="str">'POST'</span> })`),
				h("24", "", "del", `  <span class="kw">return</span> res.<span class="fn">json</span>()`),
				h("", "23", "add", `  <span class="kw">try</span> {`),
				h("", "24", "add", `    <span class="kw">const</span> res = <span class="kw">await</span> <span class="fn">fetch</span>(tokenUrl, { method: <span class="str">'POST'</span> })`),
				h("", "25", "add", `    <span class="kw">if</span> (!res.ok) <span class="kw">throw new</span> <span class="ty">OAuthError</span>(res.status)`),
				h("", "26", "add", `    <span class="kw">return</span> res.<span class="fn">json</span>()`),
				h("", "27", "add", `  } <span class="kw">catch</span> (e) {`),
				h("", "28", "add", `    logger.<span class="fn">warn</span>(<span class="str">'token exchange failed'</span>, { provider })`),
				h("", "29", "add", `    <span class="kw">throw</span> e`),
				h("", "30", "add", `  }`),
			}},
		}},
		{Hash: "9c2e1a4", Message: "Add Apple & Google OAuth providers", AuthorRole: "Coder", AgentCode: "CO", Color: cCyan, IterLabel: "iteration 1", Diff: ds(180, 0), When: "22m", Files: []domain.FileDiff{
			{Path: "src/auth/oauth.ts", Status: "a", Diff: ds(180, 0), Lines: []domain.DiffLine{
				h("", "", "hunk", "@@ -0,0 +1,180 @@ new file"),
				h("", "1", "add", `<span class="kw">export const</span> <span class="fn">appleProvider</span> = (secret) =&gt; ({`),
				h("", "2", "add", `  id: <span class="str">'apple'</span>, <span class="fn">authorize</span>, <span class="fn">exchange</span>,`),
				h("", "3", "add", `})`),
				h("", "4", "add", `<span class="kw">export const</span> <span class="fn">googleProvider</span> = (secret) =&gt; ({`),
				h("", "5", "add", `  id: <span class="str">'google'</span>, <span class="fn">authorize</span>, <span class="fn">exchange</span>,`),
				h("", "6", "add", `})`),
				h("", "7", "add", `<span class="cm">// … 174 more lines (authorize / exchange / token parsing)</span>`),
			}},
		}},
	}
}

func (s *Store) seedInbox() {
	s.inbox = []domain.InboxItem{
		{ID: "ib1", Group: "needs", Unread: true, Kind: "pause", Color: cCoral, Title: `<b>LOOP-2218</b> paused — needs your decision`, Sub: "Rate-limit the public API · over-limit behavior", Time: "12m", Action: "Answer", ActionClass: "urgent", Target: "loop:LOOP-2218", WorkspaceID: "LOOP-2218"},
		{ID: "ib2", Group: "ready", Unread: true, Kind: "pr", Color: cEmerald, Title: `<b>#482</b> is ready for review`, Sub: "Add OAuth provider config · verified in browser · checks ✓", Time: "2m", Action: "Review", ActionClass: "go", Target: "pr:482", WorkspaceID: "LOOP-2231"},
		{ID: "ib3", Group: "ready", Unread: true, Kind: "loop", Color: cCyan, Title: `Coder pushed a fix on <b>#482</b>`, Sub: "Resolved your comment on oauth.ts · re-requested review", Time: "1m", Action: "Review", ActionClass: "go", Target: "pr:482", WorkspaceID: "LOOP-2231"},
		{ID: "ib4", Group: "ready", Unread: true, Kind: "pr", Color: cEmerald, Title: `<b>#479</b> converged — ready for review`, Sub: "Stabilize flaky E2E tests · local PR · 0 findings", Time: "18m", Action: "Review", ActionClass: "go", Target: "view:pr", WorkspaceID: "LOOP-2224"},
		{ID: "ib5", Group: "upd", Unread: false, Kind: "loop", Color: cIndigo, Title: `<b>LOOP-2235</b> logged a deviation`, Sub: "Spawned an extra reviewer for a contrast pass · iteration 1", Time: "24m", Action: "View", Target: "workspace:LOOP-2235", WorkspaceID: "LOOP-2235"},
		{ID: "ib6", Group: "upd", Unread: false, Kind: "merge", Color: cIndigo, Title: `<b>#471</b> merged to main`, Sub: "Add idempotency keys to the orders API", Time: "1d", Action: "View", Target: "view:pr", WorkspaceID: "LOOP-2202"},
	}
}
