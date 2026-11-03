//! Language server processes keyed by workspace and server.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::managed;
use crate::servers::{ServerRegistry, ServerSpec};
use crate::session::{Emitter, LspEvent, Session};
use crate::uri::{canon_path, path_to_uri};

/// The editor-facing handle for one session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub server_key: String,
    pub root_uri: String,
    pub root_path: String,
    pub available: bool,
    /// Why `available` is false (soft degrade).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The `@vue/language-server` package dir for TypeScript `initialize`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vue_plugin_path: String,
}

impl SessionInfo {
    fn unavailable(reason: String) -> Self {
        Self {
            reason,
            ..Default::default()
        }
    }
}

/// Owns language server processes keyed by workspace + server.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

struct Inner {
    emit: Option<Emitter>,
    data_dir: String,
    registry: ServerRegistry,
    state: Mutex<Sessions>,
}

#[derive(Default)]
struct Sessions {
    /// Session id → session.
    sessions: HashMap<String, Arc<Session>>,
    /// `workspace\0server` → session id.
    by_key: HashMap<String, String>,
}

fn session_key(workspace_id: &str, server_key: &str) -> String {
    format!("{workspace_id}\0{server_key}")
}

fn session_info(s: &Session) -> SessionInfo {
    SessionInfo {
        id: s.id.clone(),
        server_key: s.server_key.clone(),
        root_uri: s.root_uri.clone(),
        root_path: s.root_path.clone(),
        available: true,
        reason: String::new(),
        vue_plugin_path: s.vue_plugin_path.clone(),
    }
}

impl Manager {
    /// `data_dir` is Recur's managed root for pinned installs; empty means
    /// PATH only.
    pub fn new(emit: Option<Emitter>, data_dir: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                emit,
                data_dir: data_dir.into(),
                registry: ServerRegistry::default(),
                state: Mutex::default(),
            }),
        }
    }

    /// The bundle overrides and disabled servers this manager resolves with.
    pub fn registry(&self) -> &ServerRegistry {
        &self.inner.registry
    }

    pub fn data_dir(&self) -> &str {
        &self.inner.data_dir
    }

    /// Starts or reuses the first available server for the language, or a
    /// soft-degrade info.
    pub async fn ensure(
        &self,
        workspace_id: &str,
        root_path: &str,
        language_id: &str,
        rel_path: &str,
    ) -> Result<SessionInfo, String> {
        let all = self
            .ensure_all(workspace_id, root_path, language_id, rel_path)
            .await?;
        Ok(all.into_iter().next().unwrap_or_else(|| {
            SessionInfo::unavailable(format!("no language server registered for {language_id}"))
        }))
    }

    /// Starts or reuses every enabled server for the language. Unavailable
    /// servers are left out; when none is available the result is one
    /// `available: false` info.
    pub async fn ensure_all(
        &self,
        workspace_id: &str,
        root_path: &str,
        language_id: &str,
        rel_path: &str,
    ) -> Result<Vec<SessionInfo>, String> {
        let specs = self.inner.registry.lookup_all(language_id, rel_path);
        // TypeScript pairs with Vue through the Vue package, so install it
        // even when TypeScript is the server being asked for.
        if let Some(vue) = specs
            .iter()
            .find(|s| s.key == "vue-language-server" && !s.managed.is_empty())
        {
            let _ = managed::ensure(&self.inner.data_dir, &vue.managed).await;
        }
        if specs.is_empty() {
            return Ok(vec![SessionInfo::unavailable(format!(
                "no language server registered for {language_id}"
            ))]);
        }
        let mut out = Vec::new();
        let mut last_miss = SessionInfo::default();
        for spec in specs {
            let info = self.ensure_spec(workspace_id, root_path, spec).await?;
            if info.available {
                out.push(info);
            } else {
                last_miss = info;
            }
        }
        if out.is_empty() {
            if last_miss.reason.is_empty() {
                last_miss.reason = format!("no language server available for {language_id}");
            }
            return Ok(vec![last_miss]);
        }
        Ok(out)
    }

    async fn ensure_spec(
        &self,
        workspace_id: &str,
        root_path: &str,
        mut resolved: ServerSpec,
    ) -> Result<SessionInfo, String> {
        if !resolved.managed.is_empty()
            && let Ok(path) = managed::ensure(&self.inner.data_dir, &resolved.managed).await
            && !path.is_empty()
        {
            resolved.command = path;
        }
        if !resolved.available() {
            let reason = if resolved.managed.is_empty() {
                format!("{} not found on PATH", resolved.command)
            } else {
                format!("{} not installed (and not on PATH)", resolved.managed)
            };
            return Ok(SessionInfo {
                server_key: resolved.key,
                reason,
                ..Default::default()
            });
        }
        let plugin = managed::vue_plugin_path(&self.inner.data_dir);
        let key = session_key(workspace_id, &resolved.key);

        let mut st = self.inner.state.lock();
        if let Some(id) = st.by_key.get(&key).cloned() {
            match st.sessions.get(&id).cloned() {
                Some(s) if !s.is_closed() => {
                    // simplify: restarting TS drops already-mounted .ts clients
                    // until that view remounts. Upgrade: reattach those views.
                    if resolved.key != "typescript-language-server"
                        || !s.vue_plugin_path.is_empty()
                        || plugin.is_empty()
                    {
                        return Ok(session_info(&s));
                    }
                    st.by_key.remove(&key);
                    st.sessions.remove(&id);
                    s.close();
                }
                _ => {
                    st.by_key.remove(&key);
                    st.sessions.remove(&id);
                }
            }
        }
        let id = uuid::Uuid::now_v7().to_string();
        let root = canon_path(root_path);
        let root_uri = path_to_uri(&root);
        let vue_plugin = if resolved.key == "typescript-language-server"
            || resolved.key == "vue-language-server"
        {
            plugin
        } else {
            String::new()
        };
        let s = Session::start(
            id.clone(),
            workspace_id.to_owned(),
            root,
            root_uri,
            &resolved,
            vue_plugin,
            self.wrap_emit(id.clone(), key.clone()),
        )
        .map_err(|e| format!("start {}: {e}", resolved.key))?;
        let info = session_info(&s);
        st.sessions.insert(id.clone(), s);
        st.by_key.insert(key, id);
        Ok(info)
    }

    /// Drops bookkeeping when a session exits, then forwards the event.
    fn wrap_emit(&self, id: String, key: String) -> Emitter {
        let inner: Weak<Inner> = Arc::downgrade(&self.inner);
        Arc::new(move |ev: LspEvent| {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            if matches!(ev, LspEvent::Exit { .. }) {
                let mut st = inner.state.lock();
                if st.by_key.get(&key) == Some(&id) {
                    st.by_key.remove(&key);
                }
                st.sessions.remove(&id);
            }
            if let Some(emit) = &inner.emit {
                emit(ev);
            }
        })
    }

    /// Forwards a JSON-RPC body to the session.
    pub async fn write(&self, id: &str, message: &str) -> Result<(), String> {
        let s = self.inner.state.lock().sessions.get(id).cloned();
        match s {
            Some(s) => s.write(message).await,
            None => Err("unknown lsp session".into()),
        }
    }

    /// Stops one session.
    pub fn close(&self, id: &str) {
        let s = {
            let mut st = self.inner.state.lock();
            let s = st.sessions.remove(id);
            if let Some(s) = &s {
                let key = session_key(&s.workspace_id, &s.server_key);
                if st.by_key.get(&key).map(String::as_str) == Some(id) {
                    st.by_key.remove(&key);
                }
            }
            s
        };
        if let Some(s) = s {
            s.close();
        }
    }

    /// Stops every session of a workspace.
    pub fn close_workspace(&self, workspace_id: &str) {
        self.close_where(|s| s.workspace_id == workspace_id);
    }

    /// Stops every session whose server key is listed (Settings disable).
    pub fn close_servers(&self, keys: &[String]) {
        let want: std::collections::HashSet<&str> = keys
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .collect();
        if want.is_empty() {
            return;
        }
        self.close_where(|s| want.contains(s.server_key.as_str()));
    }

    /// Stops every session (app shutdown).
    pub fn close_all(&self) {
        let victims: Vec<Arc<Session>> = {
            let mut st = self.inner.state.lock();
            st.by_key.clear();
            st.sessions.drain().map(|(_, s)| s).collect()
        };
        for s in victims {
            s.close();
        }
    }

    fn close_where(&self, pred: impl Fn(&Session) -> bool) {
        let victims: Vec<Arc<Session>> = {
            let mut st = self.inner.state.lock();
            let ids: Vec<String> = st
                .sessions
                .iter()
                .filter(|(_, s)| pred(s))
                .map(|(id, _)| id.clone())
                .collect();
            let mut out = Vec::new();
            for id in ids {
                if let Some(s) = st.sessions.remove(&id) {
                    st.by_key
                        .remove(&session_key(&s.workspace_id, &s.server_key));
                    out.push(s);
                }
            }
            out
        };
        for s in victims {
            s.close();
        }
    }

    /// Ids of the live sessions (tests and diagnostics).
    pub fn session_ids(&self) -> Vec<String> {
        self.inner.state.lock().sessions.keys().cloned().collect()
    }
}
