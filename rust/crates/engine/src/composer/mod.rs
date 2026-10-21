//! The host-side composer expansion (Go `composer.go`, `raw_regions.go`
//! `wrapComposer`): a leading `/slash` prompt action through the bundles,
//! then `@file`, `/skill`, `@terminal` and `@artifact` wrapping with raw
//! regions masked.

pub mod artifacts;
pub mod files;
pub mod raw;
pub mod skills;
pub mod terminals;

use std::path::Path;
use std::sync::LazyLock;

use recur_bundles::{Catalog, PromptAction, PromptResult, PromptRunContext, RunCtx};
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::terminals::TerminalSource;
use crate::{Engine, EngineError};

/// A leading `/slash` action ran and produced no wire prompt. Loop turns
/// treat it as on_fail; PA*/Maestro sends treat it as a no-send success.
pub const ERR_EMPTY_PROMPT_ACTION: &str = "prompt action returned no prompt";

/// Options for [`Engine::expand_composer`].
#[derive(Default, Clone)]
pub struct ExpandOpts {
    pub worktree: String,
    pub workspace_id: String,
    /// Loop turns ignore the Settings disabled-prompt list.
    pub ignore_disabled: bool,
    /// The bundle catalog to look up actions in (discovered when `None`).
    pub catalog: Option<Catalog>,
}

/// The wire prompt after the slash action and the host wrap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpandResult {
    pub prompt: String,
    pub display: String,
    pub ran_action: bool,
    pub fresh_session: bool,
}

/// A failed expansion: `result` keeps what was known (an action ran, its
/// display and fresh_session) for the caller's failure path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct ExpandError {
    pub error: EngineError,
    pub result: ExpandResult,
}

impl ExpandError {
    /// The action ran and returned nothing to send.
    pub fn is_empty_prompt_action(&self) -> bool {
        self.error.0 == ERR_EMPTY_PROMPT_ACTION
    }
}

/// A composer-leading `/name` with nothing but whitespace after it (the
/// frontend's prompt-action query rule).
static LEADING_SLASH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    recur_bundles::go::compile_regexp(r"^/([A-Za-z0-9][A-Za-z0-9_-]*)\s*$")
        .expect("leading slash pattern")
});

/// The action token of a composer that is only `/name`.
pub fn leading_prompt_slash(text: &str) -> Option<String> {
    LEADING_SLASH_TOKEN
        .captures(text.trim())
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
}

/// `@file` and `/skill` wrapping only, raw regions skipped (Go
/// `wrapComposerDialect`). `enabled` filters skill activation.
pub fn wrap_composer_dialect(
    text: &str,
    worktree: &str,
    enabled: Option<&dyn Fn(&str) -> bool>,
) -> String {
    wrap_composer(text, worktree, "", Path::new(""), enabled, None)
}

/// The full host wrap: `@file`, `/skill`, then `@terminal` snapshots and
/// `@artifact` bodies (taken from the pre-wrap composer, so a SKILL.md that
/// documents `@terminal` cannot attach anything), raw regions restored.
pub fn wrap_composer(
    text: &str,
    worktree: &str,
    workspace_id: &str,
    data_dir: &Path,
    enabled: Option<&dyn Fn(&str) -> bool>,
    terms: Option<&dyn TerminalSource>,
) -> String {
    let (masked, regions) = raw::mask_raw_regions(text);
    // Restored raw bodies may legitimately quote the markers; only the
    // scannable text loses strays.
    let masked = raw::strip_stray_raw_markers(&masked);
    let mentions = terminals::collect_terminal_mentions(&masked);
    let mut arts = artifacts::collect_artifact_mentions(&masked);
    let mut art_list = Vec::new();
    if !workspace_id.is_empty() && masked.contains('@') {
        art_list = crate::artifacts::list_artifacts(data_dir, workspace_id).unwrap_or_default();
        arts.extend(artifacts::collect_loose_artifact_title_mentions(
            &masked, &art_list,
        ));
    }
    let out = files::wrap_attached_file_mentions(&masked, worktree);
    let out = skills::wrap_activated_skills(&out, worktree, enabled);
    let out = terminals::attach_terminal_mentions(&out, &mentions, workspace_id, terms);
    let out = artifacts::attach_artifact_mentions(&out, &arts, &art_list, data_dir);
    raw::unmask_raw_regions(&out, &regions)
}

impl Engine {
    /// The one host-side expand for PA*/Maestro sends and loop specialists:
    /// a leading `[[prompt]]` slash (when it matches an action) then the
    /// host wrap. The action's mode routing is inert here: the caller
    /// already picked the role.
    pub async fn expand_composer(
        &self,
        cancel: &CancellationToken,
        text: &str,
        opts: ExpandOpts,
    ) -> Result<ExpandResult, ExpandError> {
        let raw = text.to_owned();
        let mut text = text.to_owned();
        let mut res = ExpandResult::default();
        if let Some(slash) = leading_prompt_slash(&text)
            && let Some(p) =
                self.lookup_prompt_action(&slash, opts.ignore_disabled, opts.catalog.as_ref())
        {
            res.ran_action = true;
            res.fresh_session = p.fresh_session;
            let out = self
                .run_prompt_action(cancel, &p, &opts)
                .await
                .map_err(|e| ExpandError {
                    error: e,
                    result: res.clone(),
                })?;
            res.display = out.display;
            text = out.prompt;
            if text.trim().is_empty() {
                return Err(ExpandError {
                    error: EngineError(ERR_EMPTY_PROMPT_ACTION.into()),
                    result: res,
                });
            }
        }
        let ws = opts.workspace_id.clone();
        let enabled = |name: &str| self.skill_enabled_for_workspace(&ws, name);
        let out = wrap_composer(
            &text,
            &opts.worktree,
            &opts.workspace_id,
            &self.data_dir,
            Some(&enabled),
            self.terminals.as_deref(),
        );
        if res.display.is_empty() && out != raw {
            res.display = raw;
        }
        res.prompt = out;
        Ok(res)
    }

    /// The prompt action for a slash token, unless disabled in Settings.
    pub fn lookup_prompt_action(
        &self,
        slash: &str,
        ignore_disabled: bool,
        cat: Option<&Catalog>,
    ) -> Option<PromptAction> {
        let discovered;
        let cat = match cat {
            Some(c) => c,
            None => {
                discovered = self.discover_loop_catalog().ok()?;
                &discovered
            }
        };
        let p = recur_bundles::find_prompt_by_slash(&cat.prompts, slash)?;
        if !ignore_disabled && self.settings().disabled_bundle_prompts.contains(&p.id) {
            return None;
        }
        Some(p.clone())
    }

    async fn run_prompt_action(
        &self,
        cancel: &CancellationToken,
        p: &PromptAction,
        opts: &ExpandOpts,
    ) -> Result<PromptResult, EngineError> {
        let cache = self.data_dir.join("BundleCache");
        std::fs::create_dir_all(&cache)
            .map_err(|e| EngineError(recur_bundles::go::errno_text(&e)))?;
        let rc = self.prompt_run_context(&opts.workspace_id, &opts.worktree, p);
        recur_bundles::run_prompt(
            &RunCtx::with_cancel(cancel.clone()),
            &cache.to_string_lossy(),
            p,
            rc,
        )
        .await
        .map_err(|e| EngineError(e.to_string()))
    }

    /// The run context of a `[[prompt]]` action in a workspace.
    pub fn prompt_run_context(
        &self,
        workspace_id: &str,
        worktree: &str,
        prompt: &PromptAction,
    ) -> PromptRunContext {
        let mut rc = PromptRunContext {
            workspace_id: workspace_id.to_owned(),
            worktree: worktree.to_owned(),
            work_dir: worktree.to_owned(),
            bundle_support: prompt.bundle_dir.clone(),
            tab_size: 4,
            soft_tabs: true,
            ..PromptRunContext::default()
        };
        if let Some(ws) = self.store.workspace(workspace_id) {
            rc.project_id = ws.project_id.clone();
            rc.port = ws.port;
            rc.base_branch = ws.base_branch.trim().to_owned();
            if let Some(proj) = self.store.project(&ws.project_id) {
                if rc.base_branch.is_empty() {
                    rc.base_branch = proj.base_branch.trim().to_owned();
                }
                rc.update_base_method =
                    recur_domain::effective_update_base_method(&proj, &self.settings())
                        .as_str()
                        .to_owned();
            }
        }
        rc
    }
}

#[cfg(test)]
mod tests;
