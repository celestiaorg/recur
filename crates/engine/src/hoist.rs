//! PR chat hosts and hoisting (Go `github_prs.go`, `archivePRChatHosts` ..
//! `maybeArchiveLinkedPRChatHost`): the hidden chat-only workspace the PR
//! detail dock binds to, and the move of its chats, agent tabs, artifacts,
//! findings and plan onto the live worktree once one is linked.
//!
//! A hoist reserves both ids (the hoist tombstone, exclusive with
//! Remove/Delete's deleting reservation under `hoist_mu`), waits for both
//! workspaces' chat life locks, re-validates the link, and only then moves.
//! Live turns on either side defer the hoist; their completion retries it
//! through [`Engine::maybe_archive_linked_pr_chat_host`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use recur_domain::Workspace;

use crate::Engine;
use crate::bus::{EngineEvent, WorkspaceHoisted, WorkspaceRef};
use crate::chat::tabs::pair_chat_code_from_num;
use crate::codes::{chat_turn_key, is_pair_chat_code};
use crate::core::{EngineError, Result};
use crate::prs::pr_settled;
use crate::session_config::AgentConfig;
use crate::turn::sessions::pair_chat_code_num;

/// Releases a hoist reservation on drop.
pub(crate) struct HoistGuard<'a> {
    eng: &'a Engine,
    held: Vec<String>,
}

impl Drop for HoistGuard<'_> {
    fn drop(&mut self) {
        let _hoist = self.eng.hoist_mu.lock();
        let mut set = self.eng.hoisting.lock();
        for id in &self.held {
            set.remove(id);
        }
    }
}

impl Engine {
    /// The hidden chat-only workspace for this PR, if any.
    pub fn pr_chat_host(&self, pr_id: &str) -> Option<Workspace> {
        let pr_id = pr_id.trim();
        if pr_id.is_empty() {
            return None;
        }
        self.store
            .workspaces()
            .into_iter()
            .find(|w| !w.archived && w.host_pr_id.trim() == pr_id)
    }

    /// Hides leftover chat-host rows of this PR, hoisting their chats onto
    /// `keep_id` (a live workspace that just got linked) first. Settled PRs
    /// pass `keep_id` empty and the hosts are archived in place.
    pub(crate) async fn archive_pr_chat_hosts(&self, pr_id: &str, keep_id: &str) {
        let pr_id = pr_id.trim();
        if pr_id.is_empty() {
            return;
        }
        let keep_id = keep_id.trim();
        for w in self.store.workspaces() {
            if w.archived || w.host_pr_id.trim() != pr_id || w.id == keep_id {
                continue;
            }
            self.archive_one_pr_chat_host(&w.id, keep_id).await;
        }
    }

    pub(crate) async fn archive_one_pr_chat_host(&self, host_id: &str, keep_id: &str) {
        loop {
            let Some(guard) = self.hold_pr_chat_hoist(&[host_id, keep_id]).await else {
                return;
            };
            self.wait_chat_life_idle(host_id).await;
            if !keep_id.is_empty() {
                self.wait_chat_life_idle(keep_id).await;
            }
            if !self.pr_chat_hoist_still_valid(host_id, keep_id) {
                return;
            }
            let busy = |e: &Engine| {
                e.pr_chat_host_busy(host_id)
                    || (!keep_id.is_empty() && e.pr_chat_host_busy(keep_id))
            };
            if busy(self) {
                drop(guard);
                // A turn may have finished while the tombstone hid
                // maybe_archive_linked_pr_chat_host: recheck once the
                // reservation is gone; still busy means its completion
                // retries.
                if busy(self) {
                    return;
                }
                continue;
            }
            if !keep_id.is_empty()
                && let Err(err) = self.hoist_pr_chat_host(host_id, keep_id).await
            {
                tracing::warn!("recur: hoist PR chat host {host_id} → {keep_id}: {err}");
                return;
            }
            let updated = self.store.update_workspace(host_id, |w| w.archived = true);
            drop(guard);
            if let Some(u) = updated {
                self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
            }
            return;
        }
    }

    /// Reserves `ids` for a hoist. Waits out another hoist; refuses (None)
    /// when Remove/Delete already reserved any of them, since chats must not
    /// move onto a workspace being torn down.
    pub(crate) async fn hold_pr_chat_hoist(&self, ids: &[&str]) -> Option<HoistGuard<'_>> {
        let mut held: Vec<String> = Vec::new();
        for id in ids {
            let id = id.trim();
            if !id.is_empty() && !held.iter().any(|h| h == id) {
                held.push(id.to_owned());
            }
        }
        if held.is_empty() {
            return None;
        }
        held.sort();
        loop {
            {
                let _hoist = self.hoist_mu.lock();
                let mut hoisting = self.hoisting.lock();
                if !held.iter().any(|id| hoisting.contains(id)) {
                    let deleting = self.deleting.lock();
                    if held.iter().any(|id| deleting.contains(id)) {
                        return None;
                    }
                    drop(deleting);
                    for id in &held {
                        hoisting.insert(id.clone());
                    }
                    return Some(HoistGuard { eng: self, held });
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Moves chat sessions, agent tabs, artifacts, findings and the plan
    /// from a leftover PR host onto the live worktree so Create Workspace
    /// does not orphan PR-detail chats.
    pub(crate) async fn hoist_pr_chat_host(&self, from_id: &str, to_id: &str) -> Result<()> {
        let (from_id, to_id) = (from_id.trim(), to_id.trim());
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return Ok(());
        }
        let remap = self.hoist_chat_code_remaps(from_id, to_id);
        let moved_codes = self.hoist_moved_dest_codes(from_id, &remap);
        let mut focus = {
            let f = self.chat_focus.lock();
            if f.workspace_id == from_id {
                f.code.clone()
            } else {
                String::new()
            }
        };
        if let Some(next) = remap.get(&focus).filter(|n| !n.is_empty()) {
            focus = next.clone();
        }
        let remap_map: HashMap<String, String> =
            remap.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        self.hoist_workspace_artifacts(from_id, to_id, &remap_map)?;
        self.store
            .reassign_workspace_sessions(from_id, to_id, &remap)
            .map_err(|e| EngineError(e.to_string()))?;
        self.note_chat_hoist_aliases(from_id, to_id, &remap);
        self.hoist_last_chat_surfaces(from_id, to_id, &remap);
        self.store.move_workspace_agents(from_id, to_id, &remap);
        let moved_findings = self.store.reassign_workspace_findings(from_id, to_id);
        if let Some(from) = self.store.workspace(from_id) {
            self.store.update_workspace(to_id, |w| {
                hoist_pr_chat_host_plan(w, &from, &remap);
                if from.chat_tab_order.is_empty() {
                    return;
                }
                let order = hoist_remap_codes(&from.chat_tab_order, &remap);
                if w.chat_tab_order.is_empty() {
                    w.chat_tab_order = order;
                    return;
                }
                let mut have: HashSet<String> = w.chat_tab_order.iter().cloned().collect();
                for c in order {
                    if have.insert(c.clone()) {
                        w.chat_tab_order.push(c);
                    }
                }
            });
        }
        if !focus.is_empty() {
            self.set_workspace_chat_focus(to_id, &focus);
        }
        self.sync_chat_inbox();
        if let Some(dest) = self.store.workspace(to_id) {
            self.publish_loop_updated(to_id, dest.state.clone(), &dest.phase, dest.iteration);
            self.bus
                .publish(EngineEvent::WorkspaceHoisted(WorkspaceHoisted {
                    chats_changed: true,
                    code_remap: remap.clone(),
                    focus_chat_code: focus,
                    from_workspace_id: from_id.to_owned(),
                    moved_codes: crate::prs::none_if_empty(moved_codes),
                    workspace_id: to_id.to_owned(),
                }));
        }
        if moved_findings > 0 {
            self.bus.publish(EngineEvent::FindingsUpdated(WorkspaceRef {
                workspace_id: to_id.to_owned(),
            }));
        }
        Ok(())
    }

    /// The tab codes of a workspace's agents and sessions, in Go's order
    /// (agents, then sessions).
    fn hoist_source_codes(&self, from_id: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .store
            .agents(from_id)
            .into_iter()
            .map(|a| a.code)
            .collect();
        for s in self.store.sessions_for_loop(from_id).unwrap_or_default() {
            out.push(AgentConfig::parse(&s.config_json).code);
        }
        out
    }

    /// Dest tab codes that receive a source session. Call before the
    /// sessions move (the source rows are gone after). `remap` is
    /// collision-only; identity-moved codes still belong in the set.
    fn hoist_moved_dest_codes(
        &self,
        from_id: &str,
        remap: &BTreeMap<String, String>,
    ) -> Vec<String> {
        if from_id.is_empty() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let codes = self
            .hoist_source_codes(from_id)
            .into_iter()
            .chain(remap.keys().cloned());
        for code in codes {
            let code = code.trim();
            if code.is_empty() {
                continue;
            }
            let dest = remap
                .get(code)
                .filter(|n| !n.is_empty())
                .map_or(code, String::as_str)
                .to_owned();
            if seen.insert(dest.clone()) {
                out.push(dest);
            }
        }
        out
    }

    /// Records host → dest tab keys so a send that still names the archived
    /// host follows the hoist. Only after the fallible moves, so a failed
    /// hoist does not steal routing.
    fn note_chat_hoist_aliases(
        &self,
        from_id: &str,
        to_id: &str,
        remap: &BTreeMap<String, String>,
    ) {
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return;
        }
        let mut seen = HashSet::new();
        let codes = self
            .hoist_source_codes(from_id)
            .into_iter()
            .chain(remap.keys().cloned());
        let mut aliases = self.prs.chat_hoist_alias.lock();
        for code in codes {
            let code = code.trim().to_owned();
            if code.is_empty() || !seen.insert(code.clone()) {
                continue;
            }
            let dest = remap
                .get(&code)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| code.clone());
            aliases.insert(chat_turn_key(from_id, &code), chat_turn_key(to_id, &dest));
        }
    }

    /// Follows a send that still names a hoisted host onto the live
    /// worktree tab.
    pub(crate) fn resolve_hoisted_chat(&self, workspace_id: &str, code: &str) -> (String, String) {
        let dest = self
            .prs
            .chat_hoist_alias
            .lock()
            .get(&chat_turn_key(workspace_id, code))
            .cloned();
        let unchanged = || (workspace_id.to_owned(), code.to_owned());
        let Some(dest) = dest else {
            return unchanged();
        };
        let Some((to_id, to_code)) = dest.split_once('\x00') else {
            return unchanged();
        };
        if to_id.is_empty() || to_code.is_empty() {
            return unchanged();
        }
        match self.store.workspace(to_id) {
            Some(ws) if !ws.archived => (to_id.to_owned(), to_code.to_owned()),
            _ => unchanged(),
        }
    }

    /// Rekeys Retry's looking-at cache onto dest so a host Review/Ask turn
    /// keeps its PR subject after the chat moves.
    fn hoist_last_chat_surfaces(
        &self,
        from_id: &str,
        to_id: &str,
        remap: &BTreeMap<String, String>,
    ) {
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return;
        }
        let prefix = format!("{from_id}\x00");
        let mut surfaces = self.last_chat_surfaces.lock();
        let keys: Vec<String> = surfaces
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            let Some(v) = surfaces.remove(&key) else {
                continue;
            };
            let mut code = key[prefix.len()..].to_owned();
            if let Some(next) = remap.get(&code).filter(|n| !n.is_empty()) {
                code = next.clone();
            }
            surfaces.insert(format!("{to_id}\x00{code}"), v);
        }
    }

    /// A free PA* code for each source tab whose dest code already has a
    /// transcript (or a provider session). Source codes are reserved first
    /// so a colliding PA is not remapped onto the source's own PA2.
    /// Placeholder dest PA tabs are not remapped: the agent move merges
    /// onto them.
    fn hoist_chat_code_remaps(&self, from_id: &str, to_id: &str) -> BTreeMap<String, String> {
        let mut used: HashSet<String> = HashSet::new();
        let mut occupied: HashSet<String> = HashSet::new();
        for s in self.store.sessions_for_loop(to_id).unwrap_or_default() {
            let code = AgentConfig::parse(&s.config_json).code;
            if code.is_empty() {
                continue;
            }
            used.insert(code.clone());
            if self.store.session_has_messages(&s.id) {
                occupied.insert(code);
            }
        }
        for a in self.store.agents(to_id) {
            used.insert(a.code.clone());
            if !a.session_id.trim().is_empty() {
                occupied.insert(a.code);
            }
        }
        let from_sessions = self.store.sessions_for_loop(from_id).unwrap_or_default();
        for s in &from_sessions {
            let code = AgentConfig::parse(&s.config_json).code;
            if !code.is_empty() {
                used.insert(code);
            }
        }
        for a in self.store.agents(from_id) {
            if !a.code.is_empty() {
                used.insert(a.code);
            }
        }
        let mut max = used
            .iter()
            .map(|c| pair_chat_code_num(c))
            .max()
            .unwrap_or(0)
            .max(0);
        let mut remap = BTreeMap::new();
        for s in &from_sessions {
            let code = AgentConfig::parse(&s.config_json).code;
            if code.is_empty()
                || !occupied.contains(&code)
                || remap.contains_key(&code)
                || !is_pair_chat_code(&code)
            {
                continue;
            }
            max += 1;
            let mut next = pair_chat_code_from_num(max);
            while used.contains(&next) {
                max += 1;
                next = pair_chat_code_from_num(max);
            }
            used.insert(next.clone());
            remap.insert(code, next);
        }
        remap
    }

    /// The post-reservation check: the host is still a live leftover for
    /// its PR, and `keep_id` is still that PR's live workspace (or empty
    /// because the PR settled with none). Linking is check-then-update
    /// elsewhere, so `keep_id` can go stale before the reservation.
    fn pr_chat_hoist_still_valid(&self, host_id: &str, keep_id: &str) -> bool {
        let Some(host) = self.store.workspace(host_id) else {
            return false;
        };
        if host.archived || host.host_pr_id.trim().is_empty() || host.id == keep_id {
            return false;
        }
        let Some(pr) = self.store.pr(host.host_pr_id.trim()) else {
            return false;
        };
        let live_keep = self.live_pr_workspace_id(&pr.workspace_id);
        if !keep_id.is_empty() {
            return live_keep == keep_id;
        }
        live_keep.is_empty() && pr_settled(&pr)
    }

    /// A live turn, a life-lock holder, a parked or answer-resuming
    /// ask_user, or a switch_mode remount in flight.
    pub(crate) fn pr_chat_host_busy(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        self.workspace_has_live_chat(id)
            || self.ask_users.workspace_busy(id)
            || self.workspace_has_live_continue(id)
            || self.chat_life_busy(id)
    }

    /// Hoists a leftover host onto its linked worktree once the host or the
    /// destination is idle (turn unregister, life-lock unlock, a dropped
    /// remount). Settled PRs have no destination: the host is archived in
    /// place. An open PR with only a chat host keeps it.
    ///
    /// Go ran the hoist on the caller's goroutine. Its callers here include
    /// `Drop` of a life-lock guard, which cannot wait, so the decision is
    /// made synchronously and the hoist itself runs as a tracked task.
    pub(crate) fn maybe_archive_linked_pr_chat_host(&self, workspace_id: &str) {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() || self.hoisting.lock().contains(workspace_id) {
            return;
        }
        let (pr_id, keep) = match self.store.workspace(workspace_id) {
            Some(w) if w.is_pr_chat_host() => {
                let Some(pr) = self.store.pr(w.host_pr_id.trim()) else {
                    return;
                };
                let keep = self.live_pr_workspace_id(&pr.workspace_id);
                if keep == workspace_id || (keep.is_empty() && !pr_settled(&pr)) {
                    return;
                }
                (w.host_pr_id.clone(), keep)
            }
            _ => {
                let Some(pr) = self.store.pr_by_workspace(workspace_id) else {
                    return;
                };
                let keep = self.live_pr_workspace_id(&pr.workspace_id);
                if keep.is_empty() && !pr_settled(&pr) {
                    return;
                }
                (pr.id, keep)
            }
        };
        let leftover = self
            .store
            .workspaces()
            .into_iter()
            .any(|w| !w.archived && w.host_pr_id.trim() == pr_id.trim() && w.id != keep);
        if !leftover {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let e = self.arc();
        self.tasks.teardown.spawn_on(
            async move { e.archive_pr_chat_hosts(&pr_id, &keep).await },
            &rt,
        );
    }
}

/// Copies the host's plan and definition of done onto dest when dest has
/// neither.
// simplify: an already-filed dest plan wins and the host plan stays on the
// archived row; merge the two plans if a hoisted host ever needs both.
pub(crate) fn hoist_pr_chat_host_plan(
    dest: &mut Workspace,
    from: &Workspace,
    remap: &BTreeMap<String, String>,
) {
    if dest.plan.is_some() || !dest.definition_of_done.trim().is_empty() {
        return;
    }
    if let Some(src) = &from.plan {
        let mut p = src.clone();
        for a in &mut p.attachments {
            a.path = rewrite_hoist_artifact_path(&a.path, &from.id, &dest.id);
        }
        if let Some(next) = remap.get(&p.chat_code).filter(|n| !n.is_empty()) {
            p.chat_code = next.clone();
        }
        dest.plan = Some(p);
    }
    dest.definition_of_done = from.definition_of_done.clone();
}

/// Points an artifact path under the host's artifacts dir at dest's.
pub(crate) fn rewrite_hoist_artifact_path(path: &str, from_id: &str, to_id: &str) -> String {
    let (from_id, to_id) = (from_id.trim(), to_id.trim());
    if path.is_empty() || from_id.is_empty() || to_id.is_empty() || from_id == to_id {
        return path.to_owned();
    }
    let old = format!("workspaces/{from_id}/artifacts/");
    if !path.contains(&old) {
        return path.to_owned();
    }
    path.replacen(&old, &format!("workspaces/{to_id}/artifacts/"), 1)
}

/// A tab order with `remap` applied, deduped, empties dropped.
pub(crate) fn hoist_remap_codes(order: &[String], remap: &BTreeMap<String, String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in order {
        let c = remap.get(c).filter(|n| !n.is_empty()).unwrap_or(c);
        if c.is_empty() || !seen.insert(c.clone()) {
            continue;
        }
        out.push(c.clone());
    }
    out
}
