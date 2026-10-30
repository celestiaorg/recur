//! The looking-at surface the operator and the agent share: a Recur workspace,
//! a Local PR or a forge PR. The UI prepends
//! `⟦recur-surface⟧{json}⟦/recur-surface⟧` to the wire; the engine peels it,
//! infers it for PR chat hosts, and turns it into the turn's subject note. The
//! forge-specific sentence comes from the PR's forge.

use std::sync::Arc;

use recur_domain::{Pr, PrSource, Workspace};
use recur_forge::{Forge, PrSubject};
use serde::{Deserialize, Deserializer, Serialize};

use crate::Engine;

pub const CHAT_SURFACE_WORKSPACE: &str = "workspace";
pub const CHAT_SURFACE_LOCAL_PR: &str = "local_pr";
/// A forge PR (the wire value predates other forges).
pub const CHAT_SURFACE_GITHUB_PR: &str = "github_pr";

pub const CHAT_SURFACE_OPEN: &str = "⟦recur-surface⟧";
pub const CHAT_SURFACE_CLOSE: &str = "⟦/recur-surface⟧";

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The operator's current view. The UI stamps it onto the wire; PR chat
/// hosts infer it from `host_pr_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatSurface {
    #[serde(deserialize_with = "null_default")]
    pub surface: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_id: String,
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
    pub pr_number: i64,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_source: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_url: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_title: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_base: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub pr_branch: String,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "null_default")]
    pub host: bool,
}

/// Maps the surface aliases the UI may send onto the three kinds.
pub fn normalize_chat_surface_kind(s: &str) -> &'static str {
    match s.trim().to_lowercase().as_str() {
        CHAT_SURFACE_LOCAL_PR | "local-pr" | "local" => CHAT_SURFACE_LOCAL_PR,
        CHAT_SURFACE_GITHUB_PR | "github-pr" => CHAT_SURFACE_GITHUB_PR,
        s if recur_domain::PrSource::is_github_alias(s) => CHAT_SURFACE_GITHUB_PR,
        _ => CHAT_SURFACE_WORKSPACE,
    }
}

fn trim_left_ws(s: &str) -> &str {
    s.trim_start_matches([' ', '\t', '\r', '\n'])
}

/// Strips a leading looking-at marker from the wire. `None` when there is
/// none or it is invalid (then the text is left as is).
pub fn peel_chat_surface(text: &str) -> Option<(String, ChatSurface)> {
    let s = trim_left_ws(text).strip_prefix(CHAT_SURFACE_OPEN)?;
    let i = s.find(CHAT_SURFACE_CLOSE)?;
    let raw = s[..i].trim();
    let rest = trim_left_ws(&s[i + CHAT_SURFACE_CLOSE.len()..]);
    let mut surf: ChatSurface = serde_json::from_str(raw).ok()?;
    if surf.surface.trim().is_empty() {
        return None;
    }
    surf.surface = normalize_chat_surface_kind(&surf.surface).to_owned();
    Some((rest.to_owned(), surf))
}

/// Fills the surface's PR fields the UI left empty.
pub fn merge_pr_into_surface(mut s: ChatSurface, pr: &Pr) -> ChatSurface {
    if s.pr_id.trim().is_empty() {
        s.pr_id = pr.id.clone();
    }
    if s.pr_number == 0 {
        s.pr_number = pr.number;
    }
    if s.pr_source.trim().is_empty() {
        s.pr_source = pr.source.as_str().to_owned();
    }
    if s.pr_url.trim().is_empty() {
        s.pr_url = pr.url.clone();
    }
    if s.pr_title.trim().is_empty() {
        s.pr_title = pr.title.clone();
    }
    if s.pr_base.trim().is_empty() {
        s.pr_base = pr.base.clone();
    }
    if s.pr_branch.trim().is_empty() {
        s.pr_branch = pr.branch.clone();
    }
    s
}

fn pr_subject(s: &ChatSurface) -> PrSubject {
    PrSubject {
        number: s.pr_number,
        url: s.pr_url.clone(),
        title: s.pr_title.clone(),
        branch: s.pr_branch.clone(),
        base: s.pr_base.clone(),
        host: s.host,
    }
}

/// The Local PR subject note.
pub fn local_pr_subject_prompt(s: &ChatSurface) -> String {
    let mut b = String::from("The operator is looking at Local PR");
    if s.pr_number > 0 {
        b.push_str(&format!(" #{}", s.pr_number));
    }
    let title = s.pr_title.trim();
    if !title.is_empty() {
        b.push_str(&format!(" titled \"{title}\""));
    }
    b.push_str(". This is Recur's worktree-diff review, not a GitHub pull request. Questions and reviews default to this PR's changes versus its base.");
    b
}

impl Engine {
    /// The forge that owns a surface's PR: by its source, else the first
    /// registered forge (a PR chat host whose PR row is gone).
    fn surface_forge(&self, s: &ChatSurface) -> Option<Arc<dyn Forge>> {
        let source = s.pr_source.trim();
        if !source.is_empty()
            && let Some(f) = self.forges.for_pr_source(&PrSource::from(source))
        {
            return Some(f);
        }
        self.forges.all().into_iter().next()
    }

    /// The surface a turn targets: a PR chat host is always its forge PR;
    /// otherwise the UI's (default: the workspace), filled from the PR the
    /// workspace shows.
    pub fn resolve_chat_surface(&self, ws: &Workspace, wired: Option<&ChatSurface>) -> ChatSurface {
        let mut out = wired.cloned().unwrap_or_default();
        if ws.is_pr_chat_host() {
            out.host = true;
            out.surface = CHAT_SURFACE_GITHUB_PR.to_owned();
            if out.pr_id.trim().is_empty() {
                out.pr_id = ws.host_pr_id.trim().to_owned();
            }
        } else if wired.is_none() || out.surface.trim().is_empty() {
            out.surface = CHAT_SURFACE_WORKSPACE.to_owned();
        } else {
            out.surface = normalize_chat_surface_kind(&out.surface).to_owned();
        }
        if let Some(pr) = self.surface_pr(ws, &out) {
            out = merge_pr_into_surface(out, &pr);
        }
        out
    }

    fn surface_pr(&self, ws: &Workspace, surf: &ChatSurface) -> Option<Pr> {
        let id = surf.pr_id.trim();
        if !id.is_empty()
            && let Some(pr) = self.store.pr(id)
        {
            return Some(pr);
        }
        if ws.is_pr_chat_host()
            && let Some(pr) = self.store.pr(ws.host_pr_id.trim())
        {
            return Some(pr);
        }
        if let Some(pr) = self.store.open_pr_by_workspace(&ws.id) {
            return Some(pr);
        }
        self.store
            .pr_by_workspace(&ws.id)
            .filter(|pr| pr.state != "Merged" && pr.group != "merged")
    }

    /// The per-turn subject note for a surface.
    pub fn subject_append_prompt(&self, s: &ChatSurface) -> String {
        match s.surface.as_str() {
            CHAT_SURFACE_GITHUB_PR => match self.surface_forge(s) {
                Some(f) => f.pr_subject_prompt(&pr_subject(s)),
                None => local_pr_subject_prompt(s),
            },
            CHAT_SURFACE_LOCAL_PR => local_pr_subject_prompt(s),
            _ => self.workspace_subject_prompt(s),
        }
    }

    fn workspace_subject_prompt(&self, s: &ChatSurface) -> String {
        let mut b = String::from(
            "The operator is looking at this Recur workspace (the git worktree in cwd). Questions and reviews default to this worktree versus its merge base.",
        );
        if s.pr_number <= 0 && s.pr_id.trim().is_empty() {
            return b;
        }
        let source = PrSource::from(s.pr_source.as_str());
        let forge = (!source.is_empty() && source != PrSource::Local)
            .then(|| self.forges.for_pr_source(&source))
            .flatten();
        if let Some(f) = forge {
            b.push(' ');
            b.push_str(&f.pr_label(&pr_subject(s)));
            b.push_str(" is open for this branch: that is a different surface; only treat it as the subject when they refer to the pull request.");
        } else if source == PrSource::Local || s.pr_number > 0 {
            b.push_str(" Local PR");
            if s.pr_number > 0 {
                b.push_str(&format!(" #{}", s.pr_number));
            }
            b.push_str(" is open for this workspace: only treat it as the subject when they refer to the pull request.");
        }
        b
    }
}
