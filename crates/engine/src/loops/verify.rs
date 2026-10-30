//! Verify in a real browser (PRD §10): boot the app on the workspace's port,
//! have the QA agent drive it (Playwright MCP + evidence tools only) and settle
//! pass/fail off the recorded evidence and its report_result handshake. The dev
//! server's process group never outlives the phase.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::{AgentEvent, TurnRequest};
use recur_agent_tools::Evidence;
use recur_domain::{AgentRole, Project, VerifyStep, Workspace, WorkspaceState, verify_dev_cmd};
use recur_project::runconfig::{BootOptions, boot};
use recur_project::wsenv::Vars;
use tokio_util::sync::CancellationToken;

use crate::loops::failure::LoopError;
use crate::loops::specialists::{FeedProjector, McpGuard, turn_failure};
use crate::loops::{TurnScope, Walk};
use crate::roles::{qa_runtime_extra, qa_system_prompt, role_spec};
use crate::text::{html_escape, plural};
use crate::turn::AgentSpec;

/// How long verify waits for the dev server before failing (never hangs).
pub const VERIFY_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub const VERIFY_STATE_PASSED: &str = "passed";
pub const VERIFY_STATE_FAILED: &str = "failed";

/// A pass needs at least one step and every step passing; an empty set is
/// not a pass (an absent verify must not satisfy rail #1).
pub fn verify_passed(evidence: &[Evidence]) -> bool {
    !evidence.is_empty() && evidence.iter().all(|e| e.passed)
}

impl Walk {
    /// Boots the app, runs the QA agent, and returns whether verify passed.
    /// No dev command, a server that will not boot, or a missing handshake
    /// all fail the verify.
    pub(crate) async fn run_verify(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<bool, LoopError> {
        self.set_state(WorkspaceState::Verifying, "verifying in a real browser");
        let Some(proj) = self.eng.store.project(&self.project_id) else {
            self.record_verify_absent("project not found");
            return Ok(false);
        };
        let dev_cmd = verify_dev_cmd(&proj);
        if dev_cmd.is_empty() {
            self.record_verify_absent(
                "no dev server command configured: set the project run config so loops can verify",
            );
            return Ok(false);
        }
        self.emit_feed(
            "MA",
            "Maestro",
            "Booting the app to verify the change in a real browser.",
        );

        let mut ws_rec = self.eng.store.workspace(&self.id).unwrap_or_default();
        ws_rec.worktree_path = self.worktree.clone();
        if ws_rec.id.is_empty() {
            ws_rec = Workspace {
                id: self.id.clone(),
                project_id: self.project_id.clone(),
                worktree_path: self.worktree.clone(),
                port: 0,
                ..Workspace::default()
            };
        }
        let env = Vars::from_workspace(&ws_rec, &proj).environ(None);
        // The whole boot is bounded so a hung server cannot stall the walk.
        let boot_scope =
            TurnScope::new(cancel, Some(VERIFY_PROBE_TIMEOUT + Duration::from_secs(5)));
        let booted = boot(
            &boot_scope.token,
            &dev_cmd,
            &proj.app_url,
            BootOptions {
                work_dir: self.worktree.clone(),
                port: ws_rec.port,
                env: Some(env),
                probe_timeout: VERIFY_PROBE_TIMEOUT,
                ..BootOptions::default()
            },
        )
        .await;
        boot_scope.token.cancel();
        let booted = match booted {
            Ok(b) => b,
            Err(e) => {
                self.record_verify_absent(&format!("could not boot the app: {e}"));
                return Ok(false);
            }
        };
        self.emit_feed(
            "MA",
            "Maestro",
            &format!(
                "App booted on $RECUR_PORT={} ({}): handing to QA.",
                html_escape(&booted.port),
                html_escape(&booted.app_url)
            ),
        );
        let qa = self.run_qa(cancel, &proj, &booted.app_url).await;
        // Always tear the server (and its children) down.
        booted.stop().await;
        qa?;

        // The engine decides off the recorded steps and the report_result
        // handshake, never off the QA agent's words.
        let evidence = self.h.evidence_this_verify();
        // report_result is the handshake: evidence alone is not enough.
        let passed = self
            .eng
            .store
            .workspace(&self.id)
            .and_then(|ws| ws.test_result)
            .is_some_and(|t| t.passed);
        self.persist_verify(&evidence, passed);
        if passed {
            self.emit_feed(
                "MA",
                "Maestro",
                &format!(
                    "Verify passed: {}. All green.",
                    plural(evidence.len() as i64, "browser step", "browser steps")
                ),
            );
        } else {
            self.emit_feed("MA", "Maestro", "Verify did not pass.");
        }
        Ok(passed)
    }

    /// The QA/Browser agent over the worktree, wired to the Recur MCP
    /// (evidence) and a Playwright browser MCP, with only its role's tools.
    async fn run_qa(
        &mut self,
        cancel: &CancellationToken,
        _proj: &Project,
        app_url: &str,
    ) -> Result<(), LoopError> {
        let scope = self.turn_scope(cancel);
        let rs = self.slot_role_spec("verifier", role_spec(&AgentRole::Qa));
        let qa_id = format!("{}-{}", self.id, rs.code);
        let set = self.eng.settings();
        let (model, flags) =
            self.role_model(&rs, &set.coder_model().to_string(), &set.coder_flags());
        let goal_label = self.goal_label();
        self.eng.upsert_agent(
            &self.id,
            recur_domain::Agent {
                id: qa_id.clone(),
                workspace_id: self.id.clone(),
                code: rs.code.clone(),
                name: rs.name.clone(),
                role: "drives the live app like a user".into(),
                model: recur_domain::parse_model_ref(&model).id,
                status: "working".into(),
                color: "var(--coral)".into(),
                task: goal_label.clone(),
                cur: -1,
                ..recur_domain::Agent::default()
            },
        );
        // Best effort: without the directory QA files evidence without
        // screenshots.
        let shot_dir = recur_store::paths::screenshots_dir(self.eng.data_dir(), &self.id)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Reset the collector before QA runs.
        self.eng.begin_verify(&self.h);

        // attach_evidence is QA's only way to report: no MCP, no verify.
        let chat_code = self.chat_code();
        let (token, mcp_url) = self
            .eng
            .register_mcp(&self.id, &rs.code, &chat_code, &rs.allowed_tools, &model)
            .await?;
        let _mcp = McpGuard::new(&self.eng, token);
        let (inbox, _inject) = self.h.bind_inject(&rs.code);

        let goal = self.goal.clone();
        let mut sys = qa_system_prompt(&goal, app_url, &shot_dir);
        let mut prompt = self.apply_pending_steer(&format!("Verify in the browser: {goal}"));
        let mut display = format!("Verify in the browser: {goal_label}");
        let verifier = self.phase_role_id("verifier");
        let extra = qa_runtime_extra(app_url, &shot_dir);
        sys = self.bundle_system_prompt(&verifier, &sys, &goal, &extra);
        let (expanded, disp) = self.expand_phase_prompt(cancel, "verify", &prompt).await?;
        prompt = expanded;
        if !disp.is_empty() {
            display = disp;
        }

        let req = TurnRequest {
            workspace_id: self.id.clone(),
            chat_code: chat_code.clone(),
            agent_id: qa_id,
            role: AgentRole::Qa,
            model: model.clone(),
            flags,
            system_prompt: sys,
            allowed_tools: Some(rs.allowed_tools.clone()),
            prompt,
            display_text: display,
            worktree: self.worktree.clone().into(),
            permission_mode: set.permission_mode().into(),
            mcp_url,
            playwright: true,
            ..TurnRequest::default()
        };
        let Some(permit) = self.acquire_turn_slot(&scope.token).await else {
            return Err(scope
                .err()
                .unwrap_or_else(|| LoopError::msg(crate::loops::CONTEXT_CANCELED)));
        };
        let projector = FeedProjector::new(&self.eng, &self.id, &rs, &chat_code, &model);
        let mut spec = AgentSpec::new(req);
        spec.steer = Some(inbox);
        spec.emit = Some(Arc::new(move |ev: &AgentEvent| projector.on_event(ev)));
        let out = self.eng.run_agent(&scope.token, spec).await;
        drop(permit);
        self.eng.publish_resource();
        self.eng.update_agent_status(&self.id, &rs.code, "idle");
        if let Some(te) = turn_failure(&rs.name, &scope, &out) {
            return Err(te);
        }
        Ok(())
    }

    /// Records the QA evidence and the outcome on the loop (rail #1 and the
    /// PR read it).
    fn persist_verify(&self, evidence: &[Evidence], passed: bool) {
        let steps: Vec<VerifyStep> = evidence
            .iter()
            .map(|e| VerifyStep {
                html: html_escape(&e.step),
                passed: e.passed,
                screenshot: e.screenshot_path.clone(),
            })
            .collect();
        let state = if passed {
            VERIFY_STATE_PASSED
        } else {
            VERIFY_STATE_FAILED
        };
        let Some(updated) = self.eng.store.update_workspace(&self.id, |lp| {
            lp.verify = steps;
            lp.verify_state = state.into();
        }) else {
            return;
        };
        self.eng.publish_loop_updated(
            &self.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
    }

    /// A verify that could not run, recorded as one failing step so rail #1
    /// blocks the PR with the reason.
    fn record_verify_absent(&self, reason: &str) {
        let step = VerifyStep {
            html: html_escape(reason),
            passed: false,
            ..VerifyStep::default()
        };
        self.eng.store.update_workspace(&self.id, |lp| {
            lp.verify = vec![step];
            lp.verify_state = VERIFY_STATE_FAILED.into();
        });
        self.emit_feed(
            "MA",
            "Maestro",
            &format!("Verify could not run: {}", html_escape(reason)),
        );
    }
}
