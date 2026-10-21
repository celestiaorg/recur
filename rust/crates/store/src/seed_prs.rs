use recur_domain::{
    ATTENTION_REVIEW_REQUESTED, Check, Commit, FileDiff, FocusNote, InboxItem, PrParticipant,
    PrReaction, PrReviewer, PrSource, REVIEW_STATE_APPROVED, REVIEW_STATE_PENDING, SummaryPara,
    VerifyStep,
};
use recur_domain::{Pr, PrComment};

use crate::seed::{C_CORAL, C_CYAN, C_EMERALD, C_INDIGO, ds, h, hx};
use crate::store::State;

const MUTED_AVATAR: &str = "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)";

fn participant(login: &str, name: &str, code: &str, css: &str) -> PrParticipant {
    PrParticipant {
        login: login.into(),
        name: name.into(),
        code: code.into(),
        avatar_css: css.into(),
        ..Default::default()
    }
}

fn reaction(content: &str, count: i64, mine: bool) -> PrReaction {
    PrReaction {
        content: content.into(),
        count,
        mine,
    }
}

fn verify(html: &str) -> VerifyStep {
    VerifyStep {
        passed: true,
        html: html.into(),
        ..Default::default()
    }
}

fn check(name: &str, detail: &str) -> Check {
    Check {
        name: name.into(),
        detail: detail.into(),
        passed: true,
        status: "pass".into(),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn comment(id: &str, url: &str, code: &str, author: &str, login: &str, css: &str, time: &str, verb: &str) -> PrComment {
    PrComment {
        id: id.into(),
        url: url.into(),
        code: code.into(),
        author: author.into(),
        author_login: login.into(),
        avatar_css: css.into(),
        time: time.into(),
        verb: verb.into(),
        ..Default::default()
    }
}

pub(crate) fn seed_prs(s: &mut State) {
    let pr482 = Pr {
        id: "482".into(),
        workspace_id: "LOOP-2231".into(),
        project_id: "storefront".into(),
        group: "you".into(),
        number: 482,
        source: PrSource::GitHub,
        url: "https://github.com/acme/app/pull/482".into(),
        branch: "recur/oauth-checkout".into(),
        base: "main".into(),
        title: "Add OAuth provider config + secrets handling".into(),
        state: "In review".into(),
        diff: ds(218, 12),
        file_count: 3,
        checks_ok: true,
        when: "2m".into(),
        created_at: "2026-08-28T09:50:00Z".into(),
        updated_at: "2026-08-28T09:50:00Z".into(),
        author_login: "loopbot".into(),
        viewer_login: "avery".into(),
        viewer_can_write: true,
        review_requested: true,
        attention_reasons: vec![ATTENTION_REVIEW_REQUESTED.into()],
        participants: vec![
            participant("loopbot", "loopbot", "LO", MUTED_AVATAR),
            participant("avery", "Avery Rourke", "AR", MUTED_AVATAR),
            participant("sam", "sam", "", ""),
            participant("", "PR Responder", "PR", "background:#b08bd8;color:#0A0C10"),
            participant("", "Coder", "CO", "background:var(--cyan);color:#0A0C10"),
        ],
        reviewers: vec![
            PrReviewer {
                login: "avery".into(),
                name: "Avery Rourke".into(),
                code: "AR".into(),
                avatar_css: MUTED_AVATAR.into(),
                state: REVIEW_STATE_PENDING.into(),
                ..Default::default()
            },
            PrReviewer {
                login: "sam".into(),
                name: "sam".into(),
                state: REVIEW_STATE_APPROVED.into(),
                ..Default::default()
            },
        ],
        summary: Some(vec![
            SummaryPara {
                kind: "p".into(),
                html: "Adds <b>Apple and Google sign-in</b> to checkout. Provider config and secret handling ship in this PR; the sign-in buttons in the checkout UI follow in <code>#483</code>.".into(),
                ..Default::default()
            },
            SummaryPara {
                kind: "p".into(),
                html: "<b>What changed</b>".into(),
                ..Default::default()
            },
            SummaryPara {
                kind: "bullets".into(),
                items: vec![
                    "New OAuth provider registry in <code>auth/oauth.ts</code> for Apple + Google.".into(),
                    "Client secrets now resolved from the secrets manager at startup — never read from <code>process.env</code> or logged.".into(),
                    "Session/token refresh extracted into a reusable <code>useOAuthSession</code> hook.".into(),
                ],
                ..Default::default()
            },
            SummaryPara {
                kind: "p".into(),
                html: "<b>Key decision:</b> tokens are stored in an httpOnly cookie rather than localStorage, so they're not reachable from JS. Flagging because it changes how the client reads auth state.".into(),
                ..Default::default()
            },
        ]),
        focus: FocusNote {
            title: "Where to focus your review".into(),
            body: "The token-refresh race in <code>useOAuthSession.ts</code> (lines 5–10) is the subtle part — two tabs refreshing at once. The Reviewer added a mutex; worth a human look.".into(),
        },
        verify: Some(vec![
            verify("Booted the app, opened <b>/checkout</b> on a clean session"),
            verify("<b>Sign in with Apple</b> → consent → returned with a valid session"),
            verify("<b>Sign in with Google</b> → consent → returned with a valid session"),
            verify("Reloaded — session persisted; no token in <b>localStorage</b> or logs"),
        ]),
        checks: Some(vec![
            check("test", "1m 12s"),
            check("lint", "8s"),
            check("typecheck", "22s"),
        ]),
        comments: Some(vec![
            PrComment {
                kind: "description".into(),
                body_md: "Adds Apple and Google sign-in to checkout. Provider config and secret handling ship in this PR.".into(),
                reactions: vec![reaction("hooray", 3, false)],
                ..comment("gh-description", "https://github.com/acme/app/pull/482#issue-482001", "LO", "loopbot", "loopbot", MUTED_AVATAR, "2m", "")
            },
            PrComment {
                body_md: "Left a comment on oauth.ts about the client secret — see Files changed.".into(),
                body_html: "Left a comment on <code>oauth.ts</code> about the client secret — see Files changed.".into(),
                reactions: vec![reaction("+1", 2, false), reaction("heart", 1, true)],
                ..comment("c1", "https://github.com/acme/app/pull/482#issuecomment-1001", "AR", "Avery Rourke", "avery", MUTED_AVATAR, "2m", "commented")
            },
            PrComment {
                body_html: "Got the review notes — I'll address the client-secret comment.".into(),
                ..comment("c2", "https://github.com/acme/app/pull/482#issuecomment-1002", "MA", "Maestro", "", "background:var(--amber);color:#0A0C10", "2m", "commented")
            },
            PrComment {
                body_html: "Pushed <code>a3f91c2</code> — resolved via the secrets manager. Reviewer re-ran clean, re-requested your review.".into(),
                ..comment("c3", "https://github.com/acme/app/pull/482#issuecomment-1003", "CO", "Coder", "", "background:var(--cyan);color:#0A0C10", "now", "commented")
            },
            PrComment {
                body_md: "testing again".into(),
                ..comment("gh-review-99", "https://github.com/acme/app/pull/482#pullrequestreview-99", "AR", "Avery Rourke", "avery", MUTED_AVATAR, "1m", "commented")
            },
            PrComment {
                parent_id: "gh-review-99".into(),
                review_id: "99".into(),
                body_html: "<code>src/auth/oauth.ts:11-12</code>".into(),
                body_md: "hmm".into(),
                path: "src/auth/oauth.ts".into(),
                line: 12,
                start_line: 11,
                side: "RIGHT".into(),
                diff_hunk: "@@ -4,9 +4,9 @@ provider registry\n import { secrets } from './vault'\n const providers = []\n function register() {\n   return providers\n }\n const unused = 1\n const unused2 = 2\n const unused3 = 3\n-const secret = process.env.OAUTH_CLIENT_SECRET\n+const secret = await secrets.get('oauth/client_secret')".into(),
                thread_id: "PRRT_seed99".into(),
                reactions: vec![reaction("eyes", 1, false)],
                ..comment("gh-inline-1", "https://github.com/acme/app/pull/482#discussion_r1", "AR", "Avery Rourke", "avery", MUTED_AVATAR, "1m", "commented")
            },
        ]),
        commits: Some(pr482_commits()),
        files: Some(pr482_files()),
        ..Default::default()
    };

    s.prs = vec![
        pr482,
        Pr {
            id: "479".into(),
            workspace_id: "LOOP-2224".into(),
            project_id: "storefront".into(),
            group: "you".into(),
            number: 479,
            source: PrSource::Local,
            title: "Stabilize flaky checkout E2E tests".into(),
            branch: "recur/fix-e2e".into(),
            base: "main".into(),
            diff: ds(34, 60),
            file_count: 5,
            checks_ok: true,
            state: "In review".into(),
            when: "18m".into(),
            created_at: "2026-08-28T09:34:00Z".into(),
            updated_at: "2026-08-28T09:34:00Z".into(),
            participants: vec![participant("", "Coder", "CO", "background:var(--cyan);color:#0A0C10")],
            ..Default::default()
        },
        Pr {
            id: "471".into(),
            workspace_id: "LOOP-2202".into(),
            project_id: "storefront".into(),
            group: "merged".into(),
            number: 471,
            source: PrSource::GitHub,
            url: "https://github.com/acme/app/pull/471".into(),
            title: "Add idempotency keys to the orders API".into(),
            branch: "recur/idempotency".into(),
            base: "main".into(),
            diff: ds(156, 9),
            file_count: 7,
            checks_ok: true,
            state: "Merged".into(),
            when: "1d".into(),
            created_at: "2026-08-27T09:52:00Z".into(),
            updated_at: "2026-08-27T09:52:00Z".into(),
            ..Default::default()
        },
    ];
    // Only Local PR numbers move the sequence (GitHub numbers are gh's).
    s.pr_number_seq = s
        .prs
        .iter()
        .filter(|p| p.source != PrSource::GitHub)
        .map(|p| p.number)
        .fold(s.pr_number_seq, i64::max);
}

fn file(path: &str, status: &str, add: i64, del: i64, lines: Vec<recur_domain::DiffLine>) -> FileDiff {
    FileDiff {
        path: path.into(),
        status: status.into(),
        diff: ds(add, del),
        lines: Some(lines),
        ..Default::default()
    }
}

/// The Files-changed diff for #482.
fn pr482_files() -> Vec<FileDiff> {
    vec![
        file("src/auth/oauth.ts", "m", 96, 8, vec![
            h("", "", "hunk", "@@ -1,6 +1,9 @@ provider registry"),
            h("11", "11", "ctx", r#"<span class="kw">import</span> { secrets } <span class="kw">from</span> <span class="str">'../lib/secrets'</span>"#),
            h("12", "", "del", r#"<span class="kw">const</span> secret = process.env.<span class="nm">OAUTH_CLIENT_SECRET</span>"#),
            hx("", "12", "add", r#"<span class="kw">const</span> secret = <span class="kw">await</span> secrets.<span class="fn">get</span>(<span class="str">'oauth/client_secret'</span>)"#,
                r#"<div class="icmt"><div class="icmt-pad"><div class="cmt" style="padding:0;border:none"><div class="ca" style="background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)">AR</div><div class="cc"><div class="ch"><span class="cn">Avery Rourke</span><span class="ct">2m</span></div><div class="cm">Good — this was reading from <code>process.env</code> before. Confirm the secrets client is initialised before any provider loads?</div><span class="react" style="margin-top:8px"><span style="width:5px;height:5px;border-radius:50%;background:var(--emerald);display:inline-block"></span>Coder: yes — <code>secrets.init()</code> runs in <code>bootstrap.ts</code> before providers register. ✓ resolved</span></div></div></div></div>"#),
            h("13", "13", "ctx", r#"<span class="kw">export const</span> <span class="fn">providers</span> = {"#),
            h("", "14", "add", r#"  apple: <span class="fn">appleProvider</span>(secret),"#),
            h("", "15", "add", r#"  google: <span class="fn">googleProvider</span>(secret),"#),
            h("14", "16", "ctx", "}"),
        ]),
        file("src/hooks/useOAuthSession.ts", "a", 88, 0, vec![
            h("", "", "hunk", "@@ -0,0 +1,12 @@ new file"),
            h("", "1", "add", r#"<span class="kw">export function</span> <span class="fn">useOAuthSession</span>() {"#),
            hx("", "2", "add", r#"  <span class="kw">const</span> [session, setSession] = <span class="fn">useState</span>&lt;<span class="ty">Session</span> | <span class="kw">null</span>&gt;(<span class="kw">null</span>)"#,
                r#"<div class="flag"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4-4" stroke-linecap="round"/></svg><div><div class="ft">Reviewer flagged this region for human eyes</div><div class="fm">token refresh can race across two tabs — a mutex was added below (lines 5–10). worth confirming the lock releases on error.</div></div></div>"#),
            h("", "3", "add", r#"  <span class="kw">const</span> refreshing = <span class="fn">useRef</span>(<span class="kw">false</span>) <span class="cm">// mutex guard</span>"#),
            h("", "4", "add", ""),
            h("", "5", "add", r#"  <span class="kw">async function</span> <span class="fn">refresh</span>() {"#),
            h("", "6", "add", r#"    <span class="kw">if</span> (refreshing.current) <span class="kw">return</span>"#),
            h("", "7", "add", r#"    refreshing.current = <span class="kw">true</span>"#),
            h("", "8", "add", r#"    <span class="kw">try</span> { setSession(<span class="kw">await</span> <span class="fn">rotateTokens</span>()) }"#),
            h("", "9", "add", r#"    <span class="kw">finally</span> { refreshing.current = <span class="kw">false</span> }"#),
            h("", "10", "add", "  }"),
        ]),
        file("src/checkout/SignInButtons.tsx", "m", 34, 4, vec![
            h("", "", "hunk", "@@ -8,4 +8,12 @@ checkout buttons"),
            h("8", "8", "ctx", r#"<span class="kw">return</span> ("#),
            h("", "9", "add", r#"  &lt;<span class="ty">AppleButton</span> onClick={() =&gt; <span class="fn">signIn</span>(<span class="str">'apple'</span>)} /&gt;"#),
            h("", "10", "add", r#"  &lt;<span class="ty">GoogleButton</span> onClick={() =&gt; <span class="fn">signIn</span>(<span class="str">'google'</span>)} /&gt;"#),
            h("9", "11", "ctx", ")"),
        ]),
    ]
}

#[allow(clippy::too_many_arguments)]
fn commit(hash: &str, message: &str, role: &str, code: &str, color: &str, iter: &str, add: i64, del: i64, when: &str, files: Vec<FileDiff>) -> Commit {
    Commit {
        hash: hash.into(),
        message: message.into(),
        author_role: role.into(),
        agent_code: code.into(),
        color: color.into(),
        iter_label: iter.into(),
        diff: ds(add, del),
        when: when.into(),
        files: Some(files),
        ..Default::default()
    }
}

/// The agent-authored commits for #482 with per-commit diffs.
fn pr482_commits() -> Vec<Commit> {
    vec![
        commit("e8d34b1", "Add tests for Apple + Google providers", "Tester", "TE", C_EMERALD, "verify pass", 6, 0, "4m", vec![
            file("src/auth/__tests__/oauth.test.ts", "a", 6, 0, vec![
                h("", "", "hunk", "@@ -0,0 +1,6 @@ new file"),
                h("", "1", "add", r#"<span class="fn">test</span>(<span class="str">'apple sign-in returns a session'</span>, <span class="kw">async</span> () =&gt; {"#),
                h("", "2", "add", r#"  <span class="kw">const</span> s = <span class="kw">await</span> <span class="fn">signIn</span>(<span class="str">'apple'</span>)"#),
                h("", "3", "add", r#"  <span class="fn">expect</span>(s.user).<span class="fn">toBeDefined</span>()"#),
                h("", "4", "add", "})"),
                h("", "5", "add", r#"<span class="fn">test</span>(<span class="str">'google sign-in returns a session'</span>, <span class="kw">async</span> () =&gt; { <span class="cm">/* … */</span> })"#),
            ]),
        ]),
        commit("a3f91c2", "Resolve client secret via secrets manager", "Coder", "CO", C_CYAN, "iteration 3 · review fix", 10, 6, "6m", vec![
            file("src/auth/oauth.ts", "m", 10, 6, vec![
                h("", "", "hunk", "@@ -10,6 +10,9 @@ provider registry"),
                h("10", "10", "ctx", r#"<span class="kw">import</span> { secrets } <span class="kw">from</span> <span class="str">'../lib/secrets'</span>"#),
                h("11", "", "del", r#"<span class="kw">const</span> secret = process.env.<span class="nm">OAUTH_CLIENT_SECRET</span>"#),
                h("", "11", "add", r#"<span class="kw">const</span> secret = <span class="kw">await</span> secrets.<span class="fn">get</span>(<span class="str">'oauth/client_secret'</span>)"#),
                h("12", "12", "ctx", r#"<span class="kw">export const</span> <span class="fn">providers</span> = {"#),
            ]),
        ]),
        commit("4b7f0d2", "Harden error handling on token exchange", "Coder", "CO", C_CYAN, "iteration 2 · review fix", 28, 6, "14m", vec![
            file("src/auth/oauth.ts", "m", 28, 6, vec![
                h("", "", "hunk", "@@ -22,4 +22,12 @@ token exchange"),
                h("22", "22", "ctx", r#"<span class="kw">async function</span> <span class="fn">exchange</span>(provider, code) {"#),
                h("23", "", "del", r#"  <span class="kw">const</span> res = <span class="kw">await</span> <span class="fn">fetch</span>(tokenUrl, { method: <span class="str">'POST'</span> })"#),
                h("24", "", "del", r#"  <span class="kw">return</span> res.<span class="fn">json</span>()"#),
                h("", "23", "add", r#"  <span class="kw">try</span> {"#),
                h("", "24", "add", r#"    <span class="kw">const</span> res = <span class="kw">await</span> <span class="fn">fetch</span>(tokenUrl, { method: <span class="str">'POST'</span> })"#),
                h("", "25", "add", r#"    <span class="kw">if</span> (!res.ok) <span class="kw">throw new</span> <span class="ty">OAuthError</span>(res.status)"#),
                h("", "26", "add", r#"    <span class="kw">return</span> res.<span class="fn">json</span>()"#),
                h("", "27", "add", r#"  } <span class="kw">catch</span> (e) {"#),
                h("", "28", "add", r#"    logger.<span class="fn">warn</span>(<span class="str">'token exchange failed'</span>, { provider })"#),
                h("", "29", "add", r#"    <span class="kw">throw</span> e"#),
                h("", "30", "add", "  }"),
            ]),
        ]),
        commit("9c2e1a4", "Add Apple & Google OAuth providers", "Coder", "CO", C_CYAN, "iteration 1", 180, 0, "22m", vec![
            file("src/auth/oauth.ts", "a", 180, 0, vec![
                h("", "", "hunk", "@@ -0,0 +1,180 @@ new file"),
                h("", "1", "add", r#"<span class="kw">export const</span> <span class="fn">appleProvider</span> = (secret) =&gt; ({"#),
                h("", "2", "add", r#"  id: <span class="str">'apple'</span>, <span class="fn">authorize</span>, <span class="fn">exchange</span>,"#),
                h("", "3", "add", "})"),
                h("", "4", "add", r#"<span class="kw">export const</span> <span class="fn">googleProvider</span> = (secret) =&gt; ({"#),
                h("", "5", "add", r#"  id: <span class="str">'google'</span>, <span class="fn">authorize</span>, <span class="fn">exchange</span>,"#),
                h("", "6", "add", "})"),
                h("", "7", "add", r#"<span class="cm">// … 174 more lines (authorize / exchange / token parsing)</span>"#),
            ]),
        ]),
    ]
}

#[allow(clippy::too_many_arguments)]
fn inbox(id: &str, group: &str, unread: bool, kind: &str, color: &str, title: &str, sub: &str, time: &str, action: &str, class: &str, target: &str, ws: &str) -> InboxItem {
    InboxItem {
        id: id.into(),
        group: group.into(),
        unread,
        kind: kind.into(),
        color: color.into(),
        title: title.into(),
        sub: sub.into(),
        time: time.into(),
        action: action.into(),
        action_class: class.into(),
        target: target.into(),
        workspace_id: ws.into(),
    }
}

pub(crate) fn seed_inbox(s: &mut State) {
    s.inbox = vec![
        inbox("ib1", "needs", true, "pause", C_CORAL, "<b>LOOP-2218</b> paused — needs your decision", "Rate-limit the public API · over-limit behavior", "12m", "Answer", "urgent", "loop:LOOP-2218", "LOOP-2218"),
        inbox("ib2", "ready", true, "pr", C_EMERALD, "<b>#482</b> is ready for review", "Add OAuth provider config · verified in browser · checks ✓", "2m", "Review", "go", "pr:482", "LOOP-2231"),
        inbox("ib3", "ready", true, "loop", C_CYAN, "Coder pushed a fix on <b>#482</b>", "Resolved your comment on oauth.ts · re-requested review", "1m", "Review", "go", "pr:482", "LOOP-2231"),
        inbox("ib4", "ready", true, "pr", C_EMERALD, "<b>#479</b> converged — ready for review", "Stabilize flaky E2E tests · local PR · 0 findings", "18m", "Review", "go", "view:pr", "LOOP-2224"),
        inbox("ib5", "upd", false, "loop", C_INDIGO, "<b>LOOP-2235</b> logged a deviation", "Spawned an extra reviewer for a contrast pass · iteration 1", "24m", "View", "", "workspace:LOOP-2235", "LOOP-2235"),
        inbox("ib6", "upd", false, "merge", C_INDIGO, "<b>#471</b> merged to main", "Add idempotency keys to the orders API", "1d", "View", "", "view:pr", "LOOP-2202"),
    ]
}
