//! Default config for a brand-new database, and the demo dataset the tests
//! and the e2e fixture dump use (never written by the app itself).

use recur_domain::{
    Agent, AgentStep, AutonomyLevel, ConvPoint, DiffLine, DiffStat, Escalation, EscalationOption,
    FEED_KIND_DEVIATION, FEED_KIND_NOTE, FEED_KIND_OUTCOME, FeedItem, Flags, Group,
    MergeCommitMessage, MergeMethod, Policy, Project, Resource, Settings, SquashCommitMessage,
    UpdateBaseMethod, Workspace, WorkspaceState,
};

use crate::paths::codex_default_seed_auth_path;
use crate::store::State;

// CSS color vars.
pub(crate) const C_AMBER: &str = "var(--amber)";
pub(crate) const C_CYAN: &str = "var(--cyan)";
pub(crate) const C_EMERALD: &str = "var(--emerald)";
pub(crate) const C_CORAL: &str = "var(--coral)";
pub(crate) const C_INDIGO: &str = "var(--indigo)";

/// A diff line.
pub(crate) fn h(old: &str, new: &str, typ: &str, code: &str) -> DiffLine {
    hx(old, new, typ, code, "")
}

pub(crate) fn hx(old: &str, new: &str, typ: &str, code: &str, extra: &str) -> DiffLine {
    DiffLine {
        old: old.into(),
        new: new.into(),
        r#type: typ.into(),
        code_html: code.into(),
        extra_html: extra.into(),
    }
}

pub(crate) fn ds(add: i64, del: i64) -> DiffStat {
    DiffStat { add, del }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// Defaults plus the demo dataset.
pub(crate) fn seed(s: &mut State) {
    seed_defaults(s);
    seed_demo(s);
}

/// Whether `codex login` ran on this machine, so a fresh install reuses it
/// without enabling Codex for users who never signed in.
fn has_codex_login() -> bool {
    let p = codex_default_seed_auth_path();
    !p.is_empty() && std::fs::metadata(&p).is_ok()
}

/// The config a brand-new app needs: the global policy, agent/provider
/// settings and caps, and a zeroed resource so a clean status bar reads 0.
pub(crate) fn seed_defaults(s: &mut State) {
    s.policy = Policy {
        autonomy: AutonomyLevel::Balanced,
        ..Default::default()
    };
    // Per-mode models are fully qualified `provider/id` refs; effort
    // defaults to high.
    let high = || Flags::from([("effort".to_owned(), "high".to_owned())]);
    s.settings = Settings {
        model_ask: "claude-code/claude-sonnet-5-5".into(),
        model_plan: "claude-code/claude-sonnet-5-5".into(),
        model_agent: "claude-code/claude-sonnet-5-5".into(),
        model_review: "claude-code/claude-sonnet-5-5".into(),
        model_loop: "claude-code/claude-sonnet-5-5".into(),
        model_workspace_title: "claude-code/claude-haiku-4-5-20251001".into(),
        flags_ask: high(),
        flags_plan: high(),
        flags_agent: high(),
        flags_review: high(),
        flags_loop: high(),
        flags_workspace_title: Flags::from([("effort".to_owned(), "low".to_owned())]),
        bypass_permissions: false,
        codex_chat_gpt_auth: has_codex_login(),
        max_loops: 4,
        max_agents: 8,
        default_merge_method: MergeMethod::Merge,
        default_update_base_method: UpdateBaseMethod::Merge,
        default_squash_commit_message: SquashCommitMessage::TitleAndCommits,
        default_merge_commit_message: MergeCommitMessage::Title,
        ..Default::default()
    };
    s.resource = Resource::default();
}

fn project(id: &str, name: &str, pic: &str, color: &str, open: bool, idle: bool, repo: &str) -> Project {
    Project {
        id: id.into(),
        name: name.into(),
        pic: pic.into(),
        color: color.into(),
        open,
        idle,
        repo_path: repo.into(),
        base_branch: "main".into(),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn workspace(
    id: &str,
    project: &str,
    group: Group,
    goal: &str,
    short: &str,
    state: WorkspaceState,
    status_label: &str,
    phase: &str,
    iteration: i64,
    progress: f64,
    branch: &str,
    diff: DiffStat,
    agents: &[&str],
    last: &str,
    elapsed: &str,
) -> Workspace {
    Workspace {
        id: id.into(),
        project_id: project.into(),
        group,
        goal: goal.into(),
        short: short.into(),
        state,
        status_label: status_label.into(),
        phase: phase.into(),
        iteration,
        progress,
        autonomy: AutonomyLevel::Balanced,
        branch: branch.into(),
        diff,
        agent_codes: Some(strs(agents)),
        last: last.into(),
        elapsed_label: elapsed.into(),
        ..Default::default()
    }
}

/// The mock dataset (projects, workspaces, agents, feed, PRs, escalation,
/// inbox) plus the demo status-bar numbers.
pub(crate) fn seed_demo(s: &mut State) {
    s.projects = vec![
        project("storefront", "storefront", "S", C_CYAN, true, false, "~/code/storefront"),
        project("payments", "payments-api", "P", C_AMBER, true, false, "~/code/payments-api"),
        project("marketing", "marketing-site", "M", C_INDIGO, false, true, "~/code/marketing-site"),
    ];

    let mut w2231 = workspace(
        "LOOP-2231",
        "storefront",
        Group::Active,
        "Add Apple & Google sign-in to checkout",
        "Add Apple & Google sign-in",
        WorkspaceState::Verifying,
        "Verifying",
        "browsing the app",
        3,
        0.72,
        "recur/oauth-checkout",
        ds(218, 12),
        &["MA", "CO", "QA", "RV"],
        "QA: clicking “Sign in with Apple”…",
        "14m 09s",
    );
    w2231.convergence = Some(vec![
        ConvPoint {
            iteration: 1,
            findings: 12,
            color: C_CORAL.into(),
        },
        ConvPoint {
            iteration: 2,
            findings: 4,
            color: C_AMBER.into(),
        },
        ConvPoint {
            iteration: 3,
            findings: 0,
            color: C_EMERALD.into(),
        },
    ]);
    s.workspaces = vec![
        workspace(
            "LOOP-2218",
            "payments",
            Group::Needs,
            "Rate-limit the public API",
            "Rate-limit the public API",
            WorkspaceState::Escalated,
            "Needs you",
            "asking a question",
            6,
            0.85,
            "recur/rate-limit",
            ds(0, 0),
            &["MA"],
            "escalated: ambiguous burst-limit spec",
            "paused 12m",
        ),
        w2231,
        workspace(
            "LOOP-2229",
            "storefront",
            Group::Active,
            "Migrate analytics to Postgres 16",
            "Migrate analytics to PG16",
            WorkspaceState::Coding,
            "Coding",
            "iteration 2",
            2,
            0.45,
            "recur/pg16",
            ds(540, 110),
            &["MA", "CO"],
            "Coder: rewriting migration 0042…",
            "31m",
        ),
        workspace(
            "LOOP-2235",
            "storefront",
            Group::Active,
            "Dark mode for the dashboard",
            "Dark mode for dashboard",
            WorkspaceState::Reviewing,
            "Reviewing",
            "iteration 1",
            1,
            0.3,
            "recur/dark-mode",
            ds(96, 8),
            &["MA", "CO", "RV"],
            "Reviewer: 7 findings on contrast…",
            "8m",
        ),
        workspace(
            "LOOP-2224",
            "storefront",
            Group::Ready,
            "Fix flaky checkout E2E tests",
            "Fix flaky E2E tests",
            WorkspaceState::PrOpen,
            "Ready",
            "PR #479 open",
            5,
            1.0,
            "recur/fix-e2e",
            ds(34, 60),
            &["MA", "TE"],
            "converged · awaiting your review",
            "1h 02m",
        ),
    ];

    seed_agents(s);
    seed_feed(s);
    crate::seed_prs::seed_prs(s);
    seed_escalation(s);
    crate::seed_prs::seed_inbox(s);

    // Demo status-bar numbers (the clean default is zeroed in seed_defaults).
    s.resource = Resource {
        loops_running: 3,
        needs_you: 1,
        iters_today: 37,
        ..Default::default()
    };
}

fn step(kind: &str, text: &str) -> AgentStep {
    AgentStep {
        kind: kind.into(),
        text: text.into(),
        ..Default::default()
    }
}

fn step_state(kind: &str, text: &str, state: &str) -> AgentStep {
    AgentStep {
        state: state.into(),
        ..step(kind, text)
    }
}

#[allow(clippy::too_many_arguments)]
fn agent(
    code: &str,
    name: &str,
    role: &str,
    model: &str,
    status: &str,
    color: &str,
    task: &str,
    cur: i64,
    steps: Vec<AgentStep>,
) -> Agent {
    Agent {
        id: format!("LOOP-2231-{code}"),
        workspace_id: "LOOP-2231".into(),
        code: code.into(),
        name: name.into(),
        role: role.into(),
        model: model.into(),
        status: status.into(),
        color: color.into(),
        task: task.into(),
        cur,
        steps: Some(steps),
        ..Default::default()
    }
}

fn seed_agents(s: &mut State) {
    s.agents.insert(
        "LOOP-2231".into(),
        vec![
            agent(
                "MA",
                "Maestro",
                "orchestrator · plans & delegates",
                "opus",
                "directing",
                C_AMBER,
                "Coordinating iteration 3 → verification → PR.",
                -1,
                vec![
                    step("plan", "Decomposed task into config PR + UI PR"),
                    step("spawn", "Created Reviewer, Coder, Tester, QA on demand"),
                    step("watch", "Monitoring QA verification, then will open PRs"),
                ],
            ),
            agent(
                "CO",
                "Coder",
                "writes & edits code",
                "sonnet",
                "idle",
                C_CYAN,
                "Last: resolve reviewer finding on token refresh.",
                -1,
                vec![
                    step("read", "src/auth/oauth.ts"),
                    step("read", "src/hooks/useOAuthSession.ts"),
                    AgentStep {
                        diff: Some(ds(34, 12)),
                        ..step("edit", "src/hooks/useOAuthSession.ts")
                    },
                    step_state("run", "pnpm test auth → 6 passed", "ok"),
                    step("done", "handed back to Maestro"),
                ],
            ),
            agent(
                "QA",
                "QA / Browser",
                "drives the live app like a user",
                "sonnet",
                "working",
                C_CORAL,
                "Verify Apple & Google sign-in end to end in the browser.",
                4,
                vec![
                    step_state("run", "pnpm dev --port 5173 → ready", "run"),
                    step_state("open", "navigated to http://localhost:5173/checkout", "read"),
                    step_state("click", "“Sign in with Apple” → Apple consent screen", "ok"),
                    step_state("check", "redirect returned a valid session ✓", "ok"),
                    step_state("click", "“Sign in with Google” …", "cur"),
                ],
            ),
            agent(
                "RV",
                "Reviewer",
                "critiques diffs, files findings",
                "opus",
                "idle",
                C_INDIGO,
                "Last: re-review after iteration 3.",
                -1,
                vec![
                    step("read", "diff for #482 (+218 −12)"),
                    step("note", "iter 1: 12 findings (secrets, error handling)"),
                    step("note", "iter 3: 0 findings"),
                    step("done", "approved · re-requested human review"),
                ],
            ),
        ],
    );
}

fn feed(id: &str, code: &str, name: &str, kind: &str, text: &str, time: &str, click: &str) -> FeedItem {
    FeedItem {
        id: id.into(),
        agent_code: code.into(),
        name: name.into(),
        kind: kind.into(),
        text: text.into(),
        time: time.into(),
        click_agent: click.into(),
        ..Default::default()
    }
}

fn deviation(id: &str, text: &str, rule: &str, why: &str, time: &str) -> FeedItem {
    FeedItem {
        id: id.into(),
        is_deviation: true,
        kind: FEED_KIND_DEVIATION.into(),
        name: "Maestro".into(),
        text: text.into(),
        rule: rule.into(),
        why: why.into(),
        time: time.into(),
        ..Default::default()
    }
}

fn seed_feed(s: &mut State) {
    // Kind is stamped to match each row's classification.
    s.feed.insert(
        "LOOP-2231".into(),
        vec![
            feed("f1", "MA", "Maestro", FEED_KIND_OUTCOME, "Review converged at 0 findings — advancing to live verification before opening PR #2.", "45s", ""),
            feed("f2", "QA", "QA / Browser", FEED_KIND_OUTCOME, "Booted dev server on <code>:5173</code>, navigating to <code>/checkout</code> to test the Apple flow.", "38s", "QA"),
            feed("f3", "QA", "QA / Browser", FEED_KIND_OUTCOME, "Clicked “Sign in with Apple” → redirect OK → returned with valid session. <span style=\"color:var(--emerald)\">✓ pass</span>", "12s", "QA"),
            deviation("f4", "Spawned a second, security-focused reviewer for this iteration.", "default = one reviewer", "diff touches <code>auth/oauth.ts</code> — created on demand, then retired.", "1m"),
            feed("f5", "CO", "Coder", FEED_KIND_OUTCOME, "Resolved reviewer finding: extracted token refresh into <code>useOAuthSession</code>.", "1m", "CO"),
            feed("f6", "RV", "Reviewer", FEED_KIND_OUTCOME, "Re-reviewed diff after iteration 3 — no remaining findings. Approving.", "1m", "RV"),
            deviation("f7", "Split the task into 2 PRs instead of 1.", "default = one PR", "mixed secrets-config with UI wiring — separate PRs are easier to review.", "2m"),
            feed("f8", "TE", "Tester", FEED_KIND_NOTE, "Added 6 tests for Apple + Google providers — all green.", "3m", "TE"),
        ],
    );
}

fn seed_escalation(s: &mut State) {
    let opt = |text: &str, tag: &str, desc: &str| EscalationOption {
        text: text.into(),
        tag: tag.into(),
        desc: desc.into(),
    };
    s.escalations.insert(
        "LOOP-2218".into(),
        Escalation {
            id: "esc-2218".into(),
            workspace_id: "LOOP-2218".into(),
            status: "open".into(),
            asked_ago: "12m".into(),
            question: "When a client goes over the rate limit, what should happen?".into(),
            context: "The ticket says <i>\"limit bursts to protect the API\"</i> but doesn't define the over-limit behavior. I've built the limiter middleware and tested the under-limit path — this one decision is all that's blocking the rest. I won't guess on something that changes the API's public contract.".into(),
            options: Some(vec![
                opt("Reject with <code style=\"font-family:var(--mono)\">429</code>", "simplest", "Clear to clients and easy to reason about. Can feel harsh during legitimate spikes."),
                opt("Queue & delay", "smoothest", "Absorbs short bursts without errors. Adds latency and some memory pressure under load."),
                opt("Token bucket with refill", "recommended", "Fair, industry-standard, predictable. Slightly more to implement — I can do it in this loop."),
            ]),
            ..Default::default()
        },
    );
}
