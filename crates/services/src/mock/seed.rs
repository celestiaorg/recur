//! The mock bridge's backend state: the seed (`seed.json`, the real store
//! seed, written by `cargo run -p recur-store --bin seeddump`) plus the keys resolvers add
//! as they run (`chatsByLoop`, `editorSessions`, …).
//!
//! Workspaces and PRs live in an object arena so the two views of each
//! (`loops` / `loopById`, `prs` / `prById`) share an object only where a
//! resolver stored one object in both: the seed rows start distinct, and
//! later in-place edits through a shared object show through both views.
//! [`Clone`] keeps that sharing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::js;

const SEED_JSON: &str = include_str!("seed.json");

/// A workspace or PR object in [`Seed`]'s arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjId(usize);

#[derive(Debug, Clone, Default)]
pub struct Seed {
    objs: Vec<Value>,
    loops: Vec<ObjId>,
    loop_by_id: BTreeMap<String, ObjId>,
    prs: Vec<ObjId>,
    pr_by_id: BTreeMap<String, ObjId>,
    pub projects: Vec<Value>,
    /// `projectGroups` (absent in the dumped seed).
    pub project_groups: Vec<Value>,
    pub inbox: Vec<Value>,
    pub policy: Value,
    pub settings: Value,
    pub resource: Value,
    pub feed_by_loop: BTreeMap<String, Value>,
    pub agents_by_loop: BTreeMap<String, Value>,
    pub esc_by_loop: BTreeMap<String, Value>,
    /// Extra chat tabs per workspace (`CreateWorkspaceChat`, …).
    pub chats_by_loop: BTreeMap<String, Vec<Value>>,
    pub closed_chats_by_loop: BTreeMap<String, Vec<Value>>,
    /// Workspaces whose Loop (MA) tab was closed.
    pub hidden_maestro: BTreeSet<String>,
    pub chat_order_by_loop: BTreeMap<String, Vec<String>>,
    /// `{loopId, policyId}` keyed by `workspaceId + "\0" + code`.
    pub chat_loop_by_key: BTreeMap<String, Value>,
    /// Ask Recur tabs after the primary APP tab.
    pub app_chats: Vec<Value>,
    pub editor_sessions: BTreeMap<String, Value>,
    pub shell_state: Option<Value>,
    pub expanded_schedules: Vec<String>,
}

impl Seed {
    /// `loadSeed()`: the dumped seed, with every workspace given a mock
    /// worktree path (`/mock/worktrees/<id>`) so the Observatory lists files.
    pub fn load() -> Self {
        let raw: Value = serde_json::from_str(SEED_JSON).unwrap_or(Value::Null);
        let mut seed = Self::from_json(&raw);
        let ids: Vec<ObjId> = seed
            .loops
            .iter()
            .chain(seed.loop_by_id.values())
            .copied()
            .collect();
        for id in ids {
            let l = seed.obj_mut(id);
            if !js::truthy(js::get(l, "worktreePath")) {
                let path = format!("/mock/worktrees/{}", js::get_str(l, "id"));
                js::set(l, "worktreePath", Value::String(path));
            }
        }
        seed
    }

    /// A seed from a `seed.json`-shaped document. Rows get distinct objects,
    /// as `JSON.parse` gives them.
    pub fn from_json(raw: &Value) -> Self {
        let map_of = |key: &str| -> BTreeMap<String, Value> {
            js::get(raw, key)
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default()
        };
        let mut seed = Self {
            projects: js::items(js::get(raw, "projects")),
            project_groups: js::items(js::get(raw, "projectGroups")),
            inbox: js::items(js::get(raw, "inbox")),
            policy: js::get(raw, "policy").cloned().unwrap_or(Value::Null),
            settings: js::get(raw, "settings").cloned().unwrap_or(Value::Null),
            resource: js::get(raw, "resource").cloned().unwrap_or(Value::Null),
            feed_by_loop: map_of("feedByLoop"),
            agents_by_loop: map_of("agentsByLoop"),
            esc_by_loop: map_of("escByLoop"),
            ..Self::default()
        };
        for l in js::items(js::get(raw, "loops")) {
            let id = seed.alloc(l);
            seed.loops.push(id);
        }
        for (k, l) in map_of("loopById") {
            let id = seed.alloc(l);
            seed.loop_by_id.insert(k, id);
        }
        for p in js::items(js::get(raw, "prs")) {
            let id = seed.alloc(p);
            seed.prs.push(id);
        }
        for (k, p) in map_of("prById") {
            let id = seed.alloc(p);
            seed.pr_by_id.insert(k, id);
        }
        seed
    }

    /// Stores a new object (a JS object literal or spread).
    pub fn alloc(&mut self, v: Value) -> ObjId {
        self.objs.push(v);
        ObjId(self.objs.len() - 1)
    }

    pub fn obj(&self, id: ObjId) -> &Value {
        &self.objs[id.0]
    }

    pub fn obj_mut(&mut self, id: ObjId) -> &mut Value {
        &mut self.objs[id.0]
    }

    /// `seed.loops`, in order.
    pub fn loop_ids(&self) -> &[ObjId] {
        &self.loops
    }

    pub fn loops(&self) -> impl Iterator<Item = &Value> {
        self.loops.iter().map(|id| self.obj(*id))
    }

    /// `seed.loops = ids`.
    pub fn set_loops(&mut self, ids: Vec<ObjId>) {
        self.loops = ids;
    }

    /// `seed.loopById[id]`.
    pub fn loop_by_id(&self, id: &str) -> Option<&Value> {
        self.loop_by_id.get(id).map(|o| self.obj(*o))
    }

    pub fn loop_obj(&self, id: &str) -> Option<ObjId> {
        self.loop_by_id.get(id).copied()
    }

    pub fn loop_by_id_mut(&mut self, id: &str) -> Option<&mut Value> {
        let o = *self.loop_by_id.get(id)?;
        Some(self.obj_mut(o))
    }

    /// `seed.loopById[key] = obj`.
    pub fn set_loop_by_id(&mut self, key: &str, obj: ObjId) {
        self.loop_by_id.insert(key.to_owned(), obj);
    }

    /// The first `seed.loops` row matching `pred` (`seed.loops.find`).
    pub fn find_loop(&self, pred: impl Fn(&Value) -> bool) -> Option<ObjId> {
        self.loops.iter().copied().find(|id| pred(self.obj(*id)))
    }

    /// `seed.loops = seed.loops.map(l => l.id === id ? obj : l)`.
    pub fn replace_loop(&mut self, id: &str, obj: ObjId) {
        for i in 0..self.loops.len() {
            if js::get_str(self.obj(self.loops[i]), "id") == id {
                self.loops[i] = obj;
            }
        }
    }

    /// Adds a workspace the way specs and resolvers do: one object in both
    /// `loops` (appended) and `loopById`.
    pub fn insert_loop(&mut self, ws: Value) -> ObjId {
        let key = js::get_str(&ws, "id").to_owned();
        let id = self.alloc(ws);
        self.loops.push(id);
        self.loop_by_id.insert(key, id);
        id
    }

    /// `seed.prs`, in order.
    pub fn prs(&self) -> impl Iterator<Item = &Value> {
        self.prs.iter().map(|id| self.obj(*id))
    }

    pub fn pr_ids(&self) -> &[ObjId] {
        &self.prs
    }

    pub fn set_prs(&mut self, ids: Vec<ObjId>) {
        self.prs = ids;
    }

    /// `seed.prById[id]`.
    pub fn pr_by_id(&self, id: &str) -> Option<&Value> {
        self.pr_by_id.get(id).map(|o| self.obj(*o))
    }

    pub fn pr_obj(&self, id: &str) -> Option<ObjId> {
        self.pr_by_id.get(id).copied()
    }

    pub fn pr_by_id_mut(&mut self, id: &str) -> Option<&mut Value> {
        let o = *self.pr_by_id.get(id)?;
        Some(self.obj_mut(o))
    }

    pub fn set_pr_by_id(&mut self, key: &str, obj: ObjId) {
        self.pr_by_id.insert(key.to_owned(), obj);
    }

    /// Adds a PR as one object in both `prs` (appended) and `prById`.
    pub fn insert_pr(&mut self, pr: Value) -> ObjId {
        let key = js::get_str(&pr, "id").to_owned();
        let id = self.alloc(pr);
        self.prs.push(id);
        self.pr_by_id.insert(key, id);
        id
    }

    /// The seed workspace id for a goal.
    pub fn workspace_id_by_goal(&self, goal: &str) -> Option<String> {
        self.loops()
            .find(|l| js::get_str(l, "goal") == goal)
            .map(|l| js::get_str(l, "id").to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_stamps_mock_worktrees() {
        let s = Seed::load();
        assert_eq!(s.loops().count(), 5);
        for l in s.loops() {
            let id = js::get_str(l, "id");
            assert_eq!(
                js::get_str(l, "worktreePath"),
                format!("/mock/worktrees/{id}")
            );
            let by_id = s.loop_by_id(id).unwrap();
            assert_eq!(by_id, l);
        }
        assert_ne!(
            s.loop_obj("LOOP-2218"),
            s.find_loop(|l| js::get_str(l, "id") == "LOOP-2218")
        );
        assert_eq!(s.prs().count(), 3);
        assert!(s.pr_by_id("471").is_some());
    }

    #[test]
    fn clone_keeps_sharing() {
        let mut s = Seed::load();
        let id = s.insert_loop(serde_json::json!({"id": "WS-X", "goal": "g"}));
        let mut c = s.clone();
        js::set(c.loop_by_id_mut("WS-X").unwrap(), "goal", "h".into());
        assert_eq!(js::get_str(c.obj(id), "goal"), "h");
        assert_eq!(c.workspace_id_by_goal("h").as_deref(), Some("WS-X"));
        assert_eq!(s.workspace_id_by_goal("g").as_deref(), Some("WS-X"));
    }
}
