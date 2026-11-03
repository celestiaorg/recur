//! `EditorSessionService`: per-workspace editor UI state as one gzipped JSON snapshot,
//! `<data>/workspaces/<id>/editor-session.json.gz`, merged from patches and written
//! atomically.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::rt::{blocking, rt};
use super::{AppServices, err};
use crate::api::{EditorSession, EditorSessionPatch, EditorSessionService};
use crate::{Error, Result};

pub(crate) const EDITOR_SESSION_FILE: &str = "editor-session.json.gz";

/// A stored draft or scratch body past this is a malfunctioning caller (the
/// editor caps editable files at 16MB), not a draft worth keeping.
const MAX_EDITOR_BUFFER_BYTES: usize = 32 << 20;

/// Frontend `CODE_PANEL_KEYS`; unknown kinds are dropped so a bad patch
/// cannot pin a session that would otherwise delete.
const EDITOR_PANEL_KEYS: [&str; 9] = [
    "maestro",
    "plan",
    "findings",
    "files",
    "scm",
    "outline",
    "search",
    "refs",
    "artifacts",
];

/// One workspace's lock and decoded session, so a patch does not re-read the
/// file it wrote 400ms ago.
#[derive(Default)]
struct State {
    loaded: bool,
    /// `None` = no session on disk.
    sess: Option<EditorSession>,
}

/// The per-workspace cache, dropped on clear and on `workspace.deleted` so
/// decoded draft text does not outlive the workspace.
#[derive(Default)]
pub(crate) struct Sessions(Mutex<HashMap<String, Arc<Mutex<State>>>>);

impl Sessions {
    fn state(&self, workspace_id: &str) -> Arc<Mutex<State>> {
        self.0
            .lock()
            .entry(workspace_id.to_owned())
            .or_default()
            .clone()
    }

    pub(crate) fn forget(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        self.0.lock().remove(workspace_id);
    }

    #[cfg(test)]
    pub(crate) fn cached(&self, workspace_id: &str) -> bool {
        self.0.lock().contains_key(workspace_id)
    }
}

fn editor_session_path(root: &Path, workspace_id: &str) -> PathBuf {
    root.join("workspaces")
        .join(workspace_id)
        .join(EDITOR_SESSION_FILE)
}

/// A gzipped snapshot; `None` on any failure (absent, truncated, corrupt):
/// the worst honest outcome is "nothing to restore".
pub(crate) fn read_gz_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let f = std::fs::File::open(path).ok()?;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(f).read_to_end(&mut raw).ok()?;
    // Reads the first JSON value; trailing bytes are ignored.
    let mut de = serde_json::Deserializer::from_slice(&raw).into_iter::<T>();
    de.next()?.ok()
}

/// Writes `v` as gzipped JSON (newline included) through a temp file in `dir`
/// renamed over `name`. BestSpeed keeps the CPU cost trivial at the debounce
/// cadence.
pub(crate) fn write_gz_json<T: serde::Serialize>(
    dir: &Path,
    tmp_prefix: &str,
    name: &str,
    v: &T,
) -> std::io::Result<PathBuf> {
    let mut body = serde_json::to_string(v).map_err(std::io::Error::other)?;
    body.push('\n');
    let tmp = dir.join(format!("{tmp_prefix}{}.tmp", uuid::Uuid::new_v4().simple()));
    let res = (|| {
        let f = std::fs::File::create(&tmp)?;
        let mut zw = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        zw.write_all(body.as_bytes())?;
        zw.finish()?.sync_all()?;
        let dst = dir.join(name);
        std::fs::rename(&tmp, &dst)?;
        Ok(dst)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn sanitize_panel_key(s: &str) -> String {
    if EDITOR_PANEL_KEYS.contains(&s) {
        s.to_owned()
    } else {
        String::new()
    }
}

/// Applies one patch. `None` when the merged session is empty, which tells
/// the caller to delete the file instead of writing it.
pub fn merge_editor_session(
    sess: Option<EditorSession>,
    patch: &EditorSessionPatch,
) -> Option<EditorSession> {
    let mut sess = sess.unwrap_or_default();
    if let Some(meta) = &patch.meta {
        sess.open_files = meta.open_files.clone().filter(|f| !f.is_empty());
        sess.selected_path = meta.selected_path.clone();
        sess.split_path = meta.split_path.clone();
        sess.scratch_seq = meta.scratch_seq;
        sess.file_views = meta.file_views.clone();
        if let Some(m) = &meta.syntax_by_path {
            sess.syntax_by_path = m.clone();
        }
        if let Some(m) = &meta.syntax_by_file_type {
            sess.syntax_by_file_type = m.clone();
        }
        sess.main_surface = if meta.main_surface == "review" {
            "review".into()
        } else {
            String::new()
        };
        sess.left_kind = sanitize_panel_key(&meta.left_kind);
        sess.right_kind = sanitize_panel_key(&meta.right_kind);
        sess.left_open = meta.left_open;
        sess.right_open = meta.right_open;
    }
    for (path, buf) in &patch.buffers {
        if path.is_empty() || buf.draft.len() > MAX_EDITOR_BUFFER_BYTES {
            continue;
        }
        sess.buffers.insert(path.clone(), buf.clone());
    }
    for path in &patch.drop_buffers {
        sess.buffers.remove(path);
    }
    for (path, doc) in &patch.scratch_docs {
        if path.is_empty() || doc.content.len() > MAX_EDITOR_BUFFER_BYTES {
            continue;
        }
        sess.scratch_docs.insert(path.clone(), doc.clone());
    }
    for path in &patch.drop_scratch_docs {
        sess.scratch_docs.remove(path);
    }
    (!editor_session_empty(&sess)).then_some(sess)
}

fn editor_session_empty(s: &EditorSession) -> bool {
    s.open_files.as_ref().is_none_or(Vec::is_empty)
        && s.buffers.is_empty()
        && s.scratch_docs.is_empty()
        && s.syntax_by_path.is_empty()
        && s.syntax_by_file_type.is_empty()
        && s.main_surface != "review"
        && !has_pane_chrome(s)
}

/// Left/right kind or visibility differs from the Observatory cold default
/// (maestro + files, both open).
fn has_pane_chrome(s: &EditorSession) -> bool {
    (!s.left_kind.is_empty() && s.left_kind != "maestro")
        || (!s.right_kind.is_empty() && s.right_kind != "files")
        || s.left_open == Some(false)
        || s.right_open == Some(false)
}

impl AppServices {
    fn ensure_editor_loaded(&self, st: &mut State, workspace_id: &str) {
        if st.loaded {
            return;
        }
        st.sess = read_gz_json(&editor_session_path(self.data_dir(), workspace_id));
        st.loaded = true;
    }

    /// The stored session, or `None`.
    pub(crate) fn editor_session(&self, workspace_id: &str) -> Result<Option<EditorSession>> {
        if workspace_id.is_empty() {
            return Ok(None);
        }
        let st = self.editor.state(workspace_id);
        let mut st = st.lock();
        self.ensure_editor_loaded(&mut st, workspace_id);
        Ok(st.sess.clone())
    }

    fn update_editor_session_sync(
        &self,
        workspace_id: &str,
        patch: &EditorSessionPatch,
    ) -> Result<()> {
        if workspace_id.is_empty() {
            return Err(Error::new("workspace id is required"));
        }
        // A late patch must not recreate the data dir of a workspace that was
        // just deleted.
        if self.store().workspace(workspace_id).is_none() {
            return Err(Error::new(format!(
                "workspace {:?} not found",
                workspace_id
            )));
        }
        let st = self.editor.state(workspace_id);
        let mut st = st.lock();
        self.ensure_editor_loaded(&mut st, workspace_id);
        st.sess = merge_editor_session(st.sess.take(), patch);
        match &st.sess {
            None => self.remove_editor_session_file(workspace_id),
            Some(sess) => {
                let dir = recur_store::paths::workspace_dir(self.data_dir(), workspace_id)
                    .map_err(err)?;
                write_gz_json(&dir, ".editor-session-", EDITOR_SESSION_FILE, sess)
                    .map(|_| ())
                    .map_err(err)
            }
        }
    }

    fn clear_editor_session_sync(&self, workspace_id: &str) -> Result<()> {
        if workspace_id.is_empty() {
            return Ok(());
        }
        let res = {
            let st = self.editor.state(workspace_id);
            let mut st = st.lock();
            st.loaded = true;
            st.sess = None;
            self.remove_editor_session_file(workspace_id)
        };
        self.editor.forget(workspace_id);
        res
    }

    fn remove_editor_session_file(&self, workspace_id: &str) -> Result<()> {
        match std::fs::remove_file(editor_session_path(self.data_dir(), workspace_id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl EditorSessionService for AppServices {
    async fn clear_editor_session(&self, workspace_id: &str) -> Result<()> {
        rt!(self, workspace_id => |s| {
            blocking(move || s.clear_editor_session_sync(&workspace_id)).await
        })
    }

    async fn get_editor_session(&self, workspace_id: &str) -> Result<Option<EditorSession>> {
        rt!(self, workspace_id => |s| {
            blocking(move || s.editor_session(&workspace_id)).await
        })
    }

    async fn update_editor_session(
        &self,
        workspace_id: &str,
        patch: &EditorSessionPatch,
    ) -> Result<()> {
        rt!(self, workspace_id, patch => |s| {
            blocking(move || s.update_editor_session_sync(&workspace_id, &patch)).await
        })
    }
}
