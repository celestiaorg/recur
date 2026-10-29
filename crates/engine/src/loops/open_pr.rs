//! The PR a converged walk opens: the real diff and commits from the worktree,
//! verify evidence and a checks rollup, an engine-written summary and focus
//! note, the Observatory projection, the "ready for you" inbox row, and the
//! conductor's view of the PR thread. A rework round re-creates the PR under
//! the same id.

use std::time::Duration;

use recur_domain::{
    ATTENTION_REVIEW_REQUESTED, Check, Commit, DiffStat, FileDiff, FocusNote, ImpactItem,
    InboxItem, MetricDelta, Pr, PrComment, PrSource, SummaryPara, Workspace,
};
use recur_forge::model::{comments_reviewers, commit_subjects, participants_from_pr, single_line};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, PrRef};
use crate::loops::verify::{VERIFY_STATE_FAILED, VERIFY_STATE_PASSED};
use crate::loops::wrap_up::format_loop_elapsed;
use crate::loops::{TurnScope, Walk};
use crate::text::{html_escape, plural, truncate};
use crate::turn::oneshot::cap_github_pr_body;

/// The conductor's PR-thread context cap (runes).
pub const PR_THREAD_CONTEXT_MAX: usize = 8000;

/// The first line of the goal, capped at 80 runes.
pub fn pr_title(goal: &str) -> String {
    let g = recur_agent::display::hard_truncate_runes(&single_line(goal), 80).to_owned();
    if g.is_empty() {
        "Loop changes".into()
    } else {
        g
    }
}

/// The human-facing first paragraph of an engine-written PR body: never the
/// prompt-action recipe a display label stands for.
pub fn pr_fallback_intro(goal: &str, display: &str) -> String {
    if !display.trim().is_empty() {
        return String::new();
    }
    goal.trim().to_owned()
}

/// A factual PR description from the diff when the one-shot is empty.
pub fn pr_fallback_markdown(
    goal: &str,
    commits: &[Commit],
    file_count: i64,
    add: i64,
    del: i64,
    notes: &[String],
) -> String {
    let mut b = String::new();
    let g = goal.trim();
    if !g.is_empty() {
        b.push_str(g);
        b.push_str("\n\n");
    }
    b.push_str("## Changes\n\n");
    b.push_str(&format!(
        "- {} across {}\n",
        plural(commits.len() as i64, "commit", "commits"),
        plural(file_count, "file", "files")
    ));
    b.push_str(&format!("- Net +{add} −{del} versus the base branch\n"));
    let subjects = commit_subjects(commits);
    if !subjects.is_empty() {
        b.push_str("\n## Commits\n\n");
        for s in subjects {
            b.push_str("- ");
            b.push_str(&s);
            b.push('\n');
        }
    }
    for n in notes {
        let n = n.trim();
        if n.is_empty() {
            continue;
        }
        b.push('\n');
        b.push_str(n);
        b.push('\n');
    }
    cap_github_pr_body(b.trim())
}

/// The engine-written PR summary: what the loop did and the diff shape.
pub fn engine_summary(
    goal: &str,
    file_count: i64,
    add: i64,
    del: i64,
    commit_count: i64,
) -> Vec<SummaryPara> {
    let g = html_escape(goal.trim());
    vec![
        SummaryPara {
            kind: "p".into(),
            html: format!("This loop implemented: <b>{g}</b>."),
            ..SummaryPara::default()
        },
        SummaryPara {
            kind: "p".into(),
            html: "<b>What changed</b>".into(),
            ..SummaryPara::default()
        },
        SummaryPara {
            kind: "bullets".into(),
            items: vec![
                format!(
                    "{} across {}.",
                    plural(commit_count, "commit", "commits"),
                    plural(file_count, "file", "files")
                ),
                format!("Net diff <code>+{add} −{del}</code> on the loop branch vs its base."),
                "The change converged to 0 independent-review findings before this PR opened."
                    .into(),
            ],
            ..SummaryPara::default()
        },
        SummaryPara {
            kind: "p".into(),
            html: "<b>How it converged:</b> the Coder and an independent Reviewer iterated code↔review until the Reviewer filed zero findings; the engine — not the Coder — declared convergence.".into(),
            ..SummaryPara::default()
        },
    ]
}

/// The "where to focus your review" callout: the largest changed file, the
/// blast radius, and the health delta.
pub fn engine_focus(
    files: &[FileDiff],
    impact: &[ImpactItem],
    delta: Option<&MetricDelta>,
) -> FocusNote {
    let title = "Where to focus your review".to_owned();
    let Some(first) = files.first() else {
        return FocusNote {
            title,
            body: "No file changes were produced.".into(),
        };
    };
    let mut biggest = first;
    for f in files {
        if f.diff.add + f.diff.del > biggest.diff.add + biggest.diff.del {
            biggest = f;
        }
    }
    let mut body = format!(
        "The bulk of the change is in <code>{}</code> (+{} −{}) — start there.",
        html_escape(&biggest.path),
        biggest.diff.add,
        biggest.diff.del
    );
    let (mut sym, mut api, mut test) = (0, 0, 0);
    for it in impact {
        match it.kind.as_str() {
            "symbol" => sym += 1,
            "api" => api += 1,
            "test" => test += 1,
            _ => {}
        }
    }
    if sym + api + test > 0 {
        body.push_str(&format!(
            " Blast radius: <b>{sym}</b> defs · <b>{api}</b> refs · <b>{test}</b> tests."
        ));
    }
    if let Some(d) = delta
        && (d.loc != 0 || d.max_complexity != 0 || d.hotspots != 0)
    {
        body.push_str(&format!(
            " Health Δ: LOC {:+} · max complexity {:+} · hotspots {:+}.",
            d.loc, d.max_complexity, d.hotspots
        ));
    }
    FocusNote { title, body }
}

/// The Recur checks rollup for a Local PR: the independent review (0
/// findings at pr_open by rail #3) and the browser verify when it ran.
pub fn engine_checks(lp: &Workspace, _commit_count: i64) -> Vec<Check> {
    let mut iter = lp.iteration;
    if iter == 0 {
        iter = lp.convergence.as_ref().map_or(0, |c| c.len() as i64);
    }
    let mut checks = vec![Check {
        name: "Independent review".into(),
        detail: format!("0 findings · iter {iter}"),
        passed: true,
        ..Check::default()
    }];
    match lp.verify_state.as_str() {
        VERIFY_STATE_PASSED => checks.push(Check {
            name: "Verify in browser".into(),
            detail: format!("{} ✓", plural(lp.verify.len() as i64, "step", "steps")),
            passed: true,
            ..Check::default()
        }),
        VERIFY_STATE_FAILED => checks.push(Check {
            name: "Verify in browser".into(),
            detail: "did not pass".into(),
            passed: false,
            ..Check::default()
        }),
        _ => {}
    }
    checks
}

/// Every settled check passed (pending/skipping ones do not count).
pub fn all_checks_passed(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| c.status != "pending" && c.status != "skipping")
        .all(|c| c.passed)
}

/// Sets (or adds) the PR's description comment.
pub fn upsert_description_comment(p: &mut Pr, body: &str) {
    let body = body.trim();
    let comments = p.comments.get_or_insert_with(Vec::new);
    if let Some(c) = comments.iter_mut().find(|c| c.kind == "description") {
        c.body_md = body.to_owned();
        c.body_html.clear();
        return;
    }
    if body.is_empty() {
        return;
    }
    comments.insert(
        0,
        PrComment {
            id: "gh-description".into(),
            author: "You".into(),
            kind: "description".into(),
            body_md: body.to_owned(),
            time: "now".into(),
            ..PrComment::default()
        },
    );
}

/// The review conversation and line notes the conductor sees ("" when the
/// thread is empty).
pub fn format_pr_thread(pr: &Pr) -> String {
    let mut b = String::new();
    if pr.number > 0 {
        b.push_str(&format!("PR #{}", pr.number));
        let t = pr.title.trim();
        if !t.is_empty() {
            b.push_str(&format!(" {:?}", t));
        }
        b.push('\n');
    }
    let mut wrote = false;
    for c in pr.comments.iter().flatten() {
        if c.kind == "description" || c.kind == "action" || !c.commit_sha.is_empty() {
            continue;
        }
        let mut body = c.body_md.trim().to_owned();
        if body.is_empty() {
            body = crate::loops::maestro::strip_tags(&c.body_html)
                .trim()
                .to_owned();
        }
        if body.is_empty() {
            continue;
        }
        let mut who = c.author.trim().to_owned();
        if who.is_empty() {
            who = "comment".into();
        }
        if c.is_line_note() {
            let mut loc = c.path.trim().to_owned();
            if c.line > 0 {
                loc = if loc.is_empty() {
                    format!("line {}", c.line)
                } else {
                    format!("{loc}:{}", c.line)
                };
            }
            if !loc.is_empty() {
                who.push_str(&format!(" on {loc}"));
            }
            if c.resolved {
                who.push_str(" (resolved)");
            }
        }
        b.push_str(&format!("- {who}: {}\n", truncate(&body, 400)));
        wrote = true;
    }
    for pc in &pr.pending_comments {
        let body = pc.body.trim();
        if body.is_empty() {
            continue;
        }
        let mut loc = pc.file.trim().to_owned();
        if pc.line > 0 {
            loc = if loc.is_empty() {
                format!("line {}", pc.line)
            } else {
                format!("{loc}:{}", pc.line)
            };
        }
        if loc.is_empty() {
            b.push_str(&format!("- pending: {}\n", truncate(body, 400)));
        } else {
            b.push_str(&format!("- pending on {loc}: {}\n", truncate(body, 400)));
        }
        wrote = true;
    }
    if !wrote {
        return String::new();
    }
    if b.chars().count() > PR_THREAD_CONTEXT_MAX {
        let cut: String = b.chars().take(PR_THREAD_CONTEXT_MAX).collect();
        return format!("{cut}\n…");
    }
    b
}

/// A PR carries passing browser-verify evidence.
pub fn pr_verified(pr: &Pr) -> bool {
    match &pr.verify {
        Some(v) if !v.is_empty() => v.iter().all(|s| s.passed),
        _ => false,
    }
}

fn none_if_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

impl Engine {
    /// The loop goal of a PR's workspace ("" on a miss).
    pub fn pr_goal(&self, workspace_id: &str) -> String {
        self.store
            .workspace(workspace_id)
            .map(|l| l.goal_label().to_owned())
            .unwrap_or_default()
    }

    /// The feed line that a PR opened. `elapsed` is the loop's wall time;
    /// the Maestro wrap-up is patched on once the walk settles.
    pub fn emit_feed_pr_created(&self, workspace_id: &str, pr: &Pr, elapsed: Duration) {
        let kind = if pr.source == PrSource::Local {
            "Local PR".to_owned()
        } else {
            match self.forges.for_pr_source(&pr.source) {
                Some(f) => format!("{} PR", f.label()),
                None => "Local PR".to_owned(),
            }
        };
        let mut line = format!(
            r#"<a class="loop-pr-link" data-pr="{}">{kind} #{}</a>"#,
            html_escape(&pr.id),
            pr.number
        );
        if !elapsed.is_zero() {
            line = format!("Completed in {} · {line}", format_loop_elapsed(elapsed));
        }
        self.emit_workspace_event(workspace_id, "MA", "Maestro", &line);
    }

    /// The actionable "ready for you" inbox row of a converged PR, and its OS
    /// notification. Cleared on merge or send-back.
    pub fn emit_pr_ready_inbox(&self, pr: &Pr) {
        let goal = pr_title(&self.pr_goal(&pr.workspace_id));
        let mut sub = html_escape(&truncate(&goal, 56));
        if pr_verified(pr) {
            sub.push_str(" · verified in browser · checks ✓");
        } else {
            sub.push_str(" · 0 findings");
        }
        self.store.add_inbox_item(InboxItem {
            id: format!("ib-pr-{}", pr.id),
            group: "ready".into(),
            unread: true,
            kind: "pr".into(),
            color: "var(--emerald)".into(),
            title: format!("<b>#{}</b> is ready for review", pr.number),
            sub,
            time: "now".into(),
            action: "Review".into(),
            action_class: "go".into(),
            target: format!("pr:{}", pr.id),
            workspace_id: pr.workspace_id.clone(),
        });
        self.publish_inbox_updated();
        self.publish_notify(
            &format!("PR #{} is ready for review", pr.number),
            &truncate(&goal, 120),
            &format!("pr:{}", pr.id),
        );
    }

    /// Removes a PR's actionable inbox row.
    pub fn clear_pr_inbox(&self, pr_id: &str) {
        if self.store.remove_inbox_by_target(&format!("pr:{pr_id}")) > 0 {
            self.publish_inbox_updated();
        }
    }

    /// Deletes the workspace's other unmerged PRs so a Local fallback cannot
    /// sit beside a later forge PR (one open PR per workspace).
    pub fn drop_unmerged_sibling_prs(&self, workspace_id: &str, keep_id: &str) {
        if workspace_id.is_empty() || keep_id.is_empty() {
            return;
        }
        for p in self.store.prs() {
            if p.workspace_id != workspace_id || p.id == keep_id {
                continue;
            }
            if p.state == "Merged" || p.group == "merged" {
                continue;
            }
            if !self.store.delete_pr(&p.id) {
                continue;
            }
            self.clear_pr_inbox(&p.id);
            self.bus.publish(EngineEvent::PrRemoved(PrRef {
                pr_id: p.id.clone(),
            }));
        }
    }

    /// The avatar resolver for Local PR people (the registered forge's).
    pub(crate) fn local_pr_avatar(&self, login: &str, provided: &str) -> String {
        match self.forges.all().first() {
            Some(f) => f.avatar_url(login, provided, 56),
            None => provided.to_owned(),
        }
    }
}

impl Walk {
    /// Builds and persists the PR for a converged walk and emits pr.updated
    /// plus the ready inbox row. Git reads degrade to an empty diff rather
    /// than wedging the walk.
    pub(crate) async fn open_pr(&mut self, cancel: &CancellationToken) -> crate::Result<String> {
        let base = self.base_branch();
        // A rework round reuses the PR id and number.
        let (mut pr_id, mut pr_num, mut source) = (String::new(), 0, PrSource::Local);
        if let Some(existing) = self.eng.store.pr_by_workspace(&self.id) {
            pr_id = existing.id;
            pr_num = existing.number;
            source = existing.source;
        }
        if pr_id.is_empty() {
            pr_num = self.eng.store.next_pr_number();
            pr_id = pr_num.to_string();
        }

        let (mut files, mut add, mut del) = (Vec::new(), 0, 0);
        let mut commits = Vec::new();
        if let Some(git) = self.git.clone()
            && !self.worktree.is_empty()
        {
            match git.diff_pr(&self.worktree, &base).await {
                Ok(d) => {
                    files = d.files;
                    add = d.add;
                    del = d.del;
                }
                Err(e) => self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "Could not compute the PR diff: {}",
                        html_escape(&e.to_string())
                    ),
                ),
            }
            if let Ok(cs) = git.log_commits(&self.worktree, &base).await {
                commits = self.fill_commit_diffs(&git, cs).await;
            }
        }

        let lp = self.eng.store.workspace(&self.id).unwrap_or_default();
        let proj = self.eng.store.project(&self.project_id).unwrap_or_default();
        // The live worktree branch wins over the cached name.
        let mut branch = self.eng.sync_workspace_branch(&self.id).await;
        if branch.is_empty() {
            branch = self.branch();
        }
        self.h.set_branch(&branch);
        let forge_pr = if source != PrSource::Local {
            self.eng.store.pr_by_workspace(&self.id)
        } else {
            None
        };
        let mut notes = vec!["Loop policy bar passed — ready for review.".to_owned()];
        if lp.verify_state == VERIFY_STATE_PASSED {
            notes.push("Verified in a browser.".into());
        }
        let label = self.goal_label();
        let mut title = pr_title(&label);
        let mut body = String::new();
        if let Some(git) = &self.git
            && !self.worktree.is_empty()
        {
            let (_, diff) = git.changed_diff(&self.worktree, &base).await;
            if let Ok((t, b)) = self
                .eng
                .generate_pr_description(
                    cancel,
                    &self.id,
                    &self.worktree,
                    &diff,
                    &commit_subjects(&commits),
                )
                .await
            {
                if !t.is_empty() {
                    title = t;
                }
                body = b;
            }
        }
        if body.trim().is_empty() {
            body = pr_fallback_markdown(
                &pr_fallback_intro(&self.goal, &self.goal_display),
                &commits,
                files.len() as i64,
                add,
                del,
                &notes,
            );
        }
        let new_pr = pr_id == pr_num.to_string() && source == PrSource::Local;
        if new_pr && proj.pr_on_github {
            match self
                .eng
                .create_forge_pr(&proj, &self.worktree, &branch, &base, &title, &body, false)
                .await
            {
                Some(Ok(created)) => {
                    source = created.source;
                    pr_num = created.number;
                    pr_id = created.pr_id;
                }
                Some(Err((label, reason))) => self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "{label} PR not opened ({}) — opening a Local PR instead.",
                        html_escape(&reason)
                    ),
                ),
                None => {}
            }
        } else if let Some(fpr) = &forge_pr {
            let head = if fpr.branch.is_empty() {
                branch.clone()
            } else {
                fpr.branch.clone()
            };
            // A regular push only: a divergence must not force-push away a
            // reviewer's suggestion commit.
            if let Err((label, e)) = self
                .eng
                .push_forge_pr_head(&self.worktree, &proj.repo_path, fpr, &head, false)
                .await
            {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "Could not push the rework to {label}: {}",
                        html_escape(e.trim())
                    ),
                );
            }
            // PR.Branch stays the frozen remote head.
            branch = head;
        }

        // Observatory: findings onto the diff, impact, light checks.
        let findings = self.eng.store.findings(&self.id);
        recur_code::observe::project_findings_onto_diff(&mut files, &findings);
        let mut impact = Vec::new();
        let mut extra_checks = Vec::new();
        let mut analysis_delta = None;
        if !self.worktree.is_empty()
            && let Ok(root) = recur_code::resolve_root("", &self.worktree)
        {
            let root = root.to_string_lossy().into_owned();
            let (c, r, f) = (cancel.clone(), root.clone(), files.clone());
            impact = tokio::task::spawn_blocking(move || {
                recur_code::observe::compute_impact(&c, &r, &f)
            })
            .await
            .unwrap_or_default();
            let checks_scope = TurnScope::new(cancel, Some(Duration::from_secs(25)));
            extra_checks = recur_code::observe::run_light_checks(
                Some(checks_scope.token.clone()),
                &root,
                lp.test_result.as_ref(),
            )
            .await;
            drop(checks_scope);
            let prev = self.eng.store.latest_analysis(&self.project_id, "");
            let mut run = recur_code::observe::analyze(
                cancel,
                recur_code::observe::AnalyzeOpts {
                    project_id: self.project_id.clone(),
                    workspace_id: self.id.clone(),
                    root,
                    files: files.clone(),
                    test_result: lp.test_result.clone(),
                    run_checks: false,
                    previous: prev,
                },
            )
            .await;
            run.impact = impact.clone();
            run.checks = extra_checks.clone();
            analysis_delta = run.delta.clone();
            self.eng.store.add_analysis(run);
        }

        let mut checks = engine_checks(&lp, commits.len() as i64);
        checks.extend(extra_checks);
        let mut checks_ok = all_checks_passed(&checks);
        // Forge PRs show the forge's check runs, not Recur-local ones.
        if source != PrSource::Local {
            let (c, ok) = self
                .eng
                .forge_pr_checks(&source, &proj.repo_path, pr_num)
                .await;
            checks = c;
            checks_ok = ok;
        }

        let file_count = files.len() as i64;
        let commit_count = commits.len() as i64;
        let mut pr = Pr {
            id: pr_id.clone(),
            workspace_id: self.id.clone(),
            project_id: self.project_id.clone(),
            group: "you".into(),
            number: pr_num,
            source: source.clone(),
            branch,
            base,
            title,
            summary: Some(engine_summary(&label, file_count, add, del, commit_count)),
            focus: engine_focus(&files, &impact, analysis_delta.as_ref()),
            state: "In review".into(),
            diff: DiffStat { add, del },
            file_count,
            checks_ok,
            has_checks: !checks.is_empty(),
            when: "now".into(),
            commits: none_if_empty(commits),
            files: none_if_empty(files),
            comments: self.preserved_comments(&pr_id),
            verify: none_if_empty(lp.verify.clone()),
            checks: none_if_empty(checks),
            impact,
            ..Pr::default()
        };
        if source == PrSource::Local {
            pr.attention_reasons = vec![ATTENTION_REVIEW_REQUESTED.into()];
            if recur_forge::model::pr_description(&pr).is_empty() {
                upsert_description_comment(&mut pr, &body);
            }
        }
        let eng = self.eng.clone();
        let avatar = move |l: &str, p: &str| eng.local_pr_avatar(l, p);
        pr.participants = participants_from_pr(&pr, &avatar);
        pr.reviewers = comments_reviewers(pr.comments.as_deref().unwrap_or_default(), &avatar);

        let saved = self
            .eng
            .store
            .add_pr(pr)
            .map_err(|e| crate::EngineError(format!("persist PR: {e}")))?;
        self.eng.drop_unmerged_sibling_prs(&self.id, &saved.id);
        let elapsed = self.started_at.map(|t| t.elapsed()).unwrap_or_default();
        self.eng.emit_feed_pr_created(&self.id, &saved, elapsed);
        self.eng.publish_pr_updated(&saved.id);
        // "Ready for you" is actionable; ordinary progress stays in the feed.
        self.eng.emit_pr_ready_inbox(&saved);
        Ok(saved.id)
    }

    /// Each commit's own file diff for the Commits tab (best effort).
    async fn fill_commit_diffs(
        &self,
        git: &recur_git::Manager,
        mut commits: Vec<Commit>,
    ) -> Vec<Commit> {
        for c in &mut commits {
            if let Ok(cf) = git.commit_diff(&self.worktree, &c.hash).await {
                c.files = none_if_empty(cf);
            }
            if c.when.is_empty() {
                c.when = "now".into();
            }
        }
        commits
    }

    /// A rework round keeps the existing PR's conversation.
    fn preserved_comments(&self, pr_id: &str) -> Option<Vec<PrComment>> {
        self.eng
            .store
            .pr(pr_id)
            .and_then(|p| p.comments)
            .filter(|c| !c.is_empty())
    }
}
