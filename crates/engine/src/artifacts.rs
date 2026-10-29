//! Workspace artifacts on disk:
//! `<data>/workspaces/<id>/artifacts/<uuidv7>.<ext>` plus `<uuidv7>.meta.json`,
//! outside the git worktree. Create, list, read, present, rename, delete,
//! absolute path, and hoisting between workspaces.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use recur_agent_tools::CreateArtifactRequest;
use recur_domain::path;
use recur_domain::{Artifact, ArtifactKind};
use serde_json::Value;

use crate::bus::{ArtifactEvent, EngineEvent};
use crate::{Engine, EngineError};

/// Caps `@artifact` mention snapshots (read_artifact reads the whole body).
pub const MAX_ARTIFACT_BYTES: usize = 32 * 1024;

/// The file extension of a kind.
pub fn artifact_ext(kind: &ArtifactKind) -> Option<&'static str> {
    Some(match kind {
        ArtifactKind::Html => ".html",
        ArtifactKind::Svg => ".svg",
        ArtifactKind::Mermaid => ".mmd",
        ArtifactKind::Png => ".png",
        ArtifactKind::Jpeg => ".jpg",
        ArtifactKind::Csv => ".csv",
        ArtifactKind::Json => ".json",
        ArtifactKind::Md => ".md",
        _ => return None,
    })
}

/// A supported kind, case-insensitive.
pub fn parse_artifact_kind(s: &str) -> Result<ArtifactKind, EngineError> {
    let k = ArtifactKind::from(s.trim().to_lowercase().as_str());
    if artifact_ext(&k).is_none() {
        return Err(EngineError(format!("unsupported artifact kind {:?}", s)));
    }
    Ok(k)
}

/// PNG/JPEG bodies are not text.
pub fn artifact_kind_is_binary(k: &ArtifactKind) -> bool {
    matches!(k, ArtifactKind::Png | ArtifactKind::Jpeg)
}

/// RFC 3339 UTC with nanoseconds: trailing fraction zeros trimmed, no fraction
/// when it is zero.
fn rfc3339_nano_utc(t: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Timelike;
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let frac = format!("{:09}", t.nanosecond() % 1_000_000_000);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{base}Z")
    } else {
        format!("{base}.{frac}Z")
    }
}

/// Two-space indented JSON.
pub fn marshal_indent<T: serde::Serialize>(v: &T) -> Option<String> {
    serde_json::to_string_pretty(v).ok()
}

fn io_err(e: std::io::Error) -> EngineError {
    EngineError(e.to_string())
}

fn path_err(path: &Path, e: &std::io::Error) -> EngineError {
    EngineError(format!("{}: {e}", path.display()))
}

/// Every artifact of a workspace, newest first (ties by id, descending).
/// Read-only: a workspace that never created one leaves no directory.
pub fn list_artifacts(data_dir: &Path, workspace_id: &str) -> Result<Vec<Artifact>, EngineError> {
    if workspace_id.is_empty() {
        return Ok(Vec::new());
    }
    let dir = data_dir
        .join("workspaces")
        .join(workspace_id)
        .join("artifacts");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(path_err(&dir, &e)),
    };
    let mut out: Vec<Artifact> = Vec::new();
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if ent.file_type().is_ok_and(|t| t.is_dir()) || !name.ends_with(".meta.json") {
            continue;
        }
        let Ok(b) = std::fs::read(ent.path()) else {
            continue;
        };
        if let Ok(a) = serde_json::from_slice::<Artifact>(&b) {
            out.push(a);
        }
    }
    out.sort_by(|a, b| {
        let (ta, tb) = (artifact_time(&a.created_at), artifact_time(&b.created_at));
        tb.cmp(&ta).then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// An RFC 3339 time; `None` when unparsable.
fn artifact_time(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok()
}

/// One artifact by id.
pub fn find_artifact(
    data_dir: &Path,
    workspace_id: &str,
    artifact_id: &str,
) -> Result<Artifact, EngineError> {
    let artifact_id = artifact_id.trim();
    let list = list_artifacts(data_dir, workspace_id)?;
    list.into_iter()
        .find(|a| !artifact_id.is_empty() && a.id == artifact_id)
        .ok_or_else(|| EngineError(format!("artifact not found: {artifact_id}")))
}

/// The artifact's file, refused when its stored path leaves the workspace's
/// artifacts dir.
pub fn artifact_abs_path(data_dir: &Path, a: &Artifact) -> Result<PathBuf, EngineError> {
    let rel = a.path.trim_start_matches('/');
    let full = PathBuf::from(path::join(&[&data_dir.to_string_lossy(), rel]));
    let dir = recur_store::paths::artifacts_dir(data_dir, &a.workspace_id).map_err(io_err)?;
    let dir_s = dir.to_string_lossy().into_owned();
    let full_s = full.to_string_lossy();
    if !full_s.starts_with(&format!("{dir_s}/")) && full_s != dir_s {
        return Err(EngineError("artifact path outside workspace".into()));
    }
    Ok(full)
}

fn artifact_meta_path(data_dir: &Path, a: &Artifact) -> Result<PathBuf, EngineError> {
    let full = artifact_abs_path(data_dir, a)?;
    let dir = full.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(dir.join(format!("{}.meta.json", a.id)))
}

/// A text artifact's body. `max_bytes` 0 reads it whole; otherwise the body
/// is cut on a rune boundary and marked truncated.
pub fn read_artifact_text_limit(
    data_dir: &Path,
    a: &Artifact,
    max_bytes: usize,
) -> Result<String, EngineError> {
    use std::io::Read;
    let full = artifact_abs_path(data_dir, a)?;
    let mut f = std::fs::File::open(&full).map_err(|e| path_err(&full, &e))?;
    let mut more = false;
    let raw = if max_bytes == 0 {
        let mut raw = Vec::new();
        f.read_to_end(&mut raw).map_err(|e| path_err(&full, &e))?;
        raw
    } else {
        // One read into a max+1 buffer.
        let mut buf = vec![0u8; max_bytes + 1];
        let n = f.read(&mut buf).map_err(|e| path_err(&full, &e))?;
        more = n > max_bytes;
        let mut n = n.min(max_bytes);
        while n > 0 && n < buf.len() && (buf[n] & 0xC0) == 0x80 {
            n -= 1;
        }
        buf.truncate(n);
        buf
    };
    let mut s = String::from_utf8(raw).map_err(|_| EngineError("not valid UTF-8".into()))?;
    if more {
        s.push_str("\n…(truncated)");
    }
    Ok(s)
}

/// The capped body for `@artifact` snapshots.
pub fn read_artifact_text(data_dir: &Path, a: &Artifact) -> Result<String, EngineError> {
    read_artifact_text_limit(data_dir, a, MAX_ARTIFACT_BYTES)
}

/// Rename when possible, else copy and remove.
fn move_artifact_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let data = std::fs::read(src)?;
    std::fs::write(dst, data)?;
    std::fs::remove_file(src)
}

/// Writes the destination metadata, then drops the source's. A metadata
/// write failure moves the content back.
pub(crate) fn commit_hoisted_artifact(
    src_file: &Path,
    dest_file: &Path,
    src_meta: &Path,
    dest_meta: &Path,
    a: &Artifact,
) -> Result<(), EngineError> {
    let Some(b) = marshal_indent(a) else {
        let _ = move_artifact_file(dest_file, src_file);
        return Err(EngineError("json: unsupported value".into()));
    };
    if let Err(e) = std::fs::write(dest_meta, b) {
        let _ = move_artifact_file(dest_file, src_file);
        return Err(path_err(dest_meta, &e));
    }
    let _ = std::fs::remove_file(src_meta);
    Ok(())
}

impl Engine {
    fn publish_artifact_created(&self, workspace_id: &str, a: &Artifact) {
        self.bus
            .publish(EngineEvent::ArtifactCreated(ArtifactEvent {
                artifact: a.clone(),
                workspace_id: workspace_id.to_owned(),
            }));
    }

    /// Host `create_artifact`: a UUIDv7 file (no same-title overwrites) plus
    /// its metadata; PNG/JPEG content is base64.
    pub fn create_artifact(
        &self,
        workspace_id: &str,
        agent_code: &str,
        req: CreateArtifactRequest,
    ) -> Result<Artifact, EngineError> {
        let kind = parse_artifact_kind(&req.kind)?;
        let dir =
            recur_store::paths::artifacts_dir(&self.data_dir, workspace_id).map_err(io_err)?;
        let id = uuid::Uuid::now_v7().to_string();
        let ext = artifact_ext(&kind).unwrap_or_default();
        let filename = format!("{id}{ext}");
        let full = dir.join(&filename);
        let data = if artifact_kind_is_binary(&kind) {
            crate::text::go_base64_decode(req.content.trim())
                .map_err(|e| EngineError(format!("png/jpeg content must be base64: {e}")))?
        } else {
            req.content.into_bytes()
        };
        std::fs::write(&full, data).map_err(|e| path_err(&full, &e))?;
        let a = Artifact {
            id: id.clone(),
            workspace_id: workspace_id.to_owned(),
            title: req.title.trim().to_owned(),
            kind,
            path: format!("workspaces/{workspace_id}/artifacts/{filename}"),
            created_at: rfc3339_nano_utc(chrono::Utc::now()),
            created_by: agent_code.to_owned(),
        };
        if let Some(b) = marshal_indent(&a) {
            let _ = std::fs::write(dir.join(format!("{id}.meta.json")), b);
        }
        self.publish_artifact_created(workspace_id, &a);
        Ok(a)
    }

    pub fn list_artifacts(&self, workspace_id: &str) -> Result<Vec<Artifact>, EngineError> {
        list_artifacts(&self.data_dir, workspace_id)
    }

    /// Host `present_artifact`: opens it in the UI.
    pub fn present_artifact(&self, workspace_id: &str, artifact_id: &str) -> String {
        let list = match self.list_artifacts(workspace_id) {
            Ok(l) => l,
            Err(e) => return format!("Could not list artifacts: {e}"),
        };
        match list.into_iter().find(|a| a.id == artifact_id) {
            Some(a) => {
                self.bus
                    .publish(EngineEvent::ArtifactPresented(ArtifactEvent {
                        artifact: a.clone(),
                        workspace_id: workspace_id.to_owned(),
                    }));
                format!("Presented artifact {} ({}).", a.id, a.title)
            }
            None => format!("Artifact not found: {artifact_id}"),
        }
    }

    /// Host `read_artifact`: metadata plus the full text body (binary kinds
    /// flagged, not inlined), as sorted-key JSON.
    pub fn read_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<String, EngineError> {
        let artifact_id = artifact_id.trim();
        if artifact_id.is_empty() {
            return Err(EngineError("artifact id is required".into()));
        }
        let list = self.list_artifacts(workspace_id)?;
        let Some(a) = list.into_iter().find(|a| a.id == artifact_id) else {
            return Err(EngineError(format!("artifact not found: {artifact_id}")));
        };
        let mut out: BTreeMap<&str, Value> = BTreeMap::new();
        out.insert("id", Value::String(a.id.clone()));
        out.insert("title", Value::String(a.title.clone()));
        out.insert("kind", Value::String(a.kind.as_str().to_owned()));
        out.insert("path", Value::String(a.path.clone()));
        if artifact_kind_is_binary(&a.kind) {
            out.insert("binary", Value::Bool(true));
        } else {
            let body = read_artifact_text_limit(&self.data_dir, &a, 0)?;
            out.insert("content", Value::String(body));
        }
        serde_json::to_string(&out).map_err(|e| EngineError(e.to_string()))
    }

    /// Retitles an artifact (republished as `artifact.created`).
    pub fn rename_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        title: &str,
    ) -> Result<Artifact, EngineError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(EngineError("title is required".into()));
        }
        let mut a = find_artifact(&self.data_dir, workspace_id, artifact_id)?;
        let meta = artifact_meta_path(&self.data_dir, &a)?;
        a.title = title.to_owned();
        let b = marshal_indent(&a).ok_or_else(|| EngineError("json: unsupported value".into()))?;
        std::fs::write(&meta, b).map_err(|e| path_err(&meta, &e))?;
        self.publish_artifact_created(workspace_id, &a);
        Ok(a)
    }

    /// Removes an artifact's metadata and file (a missing file is fine).
    pub fn delete_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<(), EngineError> {
        let a = find_artifact(&self.data_dir, workspace_id, artifact_id)?;
        let full = artifact_abs_path(&self.data_dir, &a)?;
        let meta = artifact_meta_path(&self.data_dir, &a)?;
        std::fs::remove_file(&meta).map_err(|e| path_err(&meta, &e))?;
        match std::fs::remove_file(&full) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(path_err(&full, &e)),
            _ => Ok(()),
        }
    }

    /// The artifact's file on disk (Reveal in Finder).
    pub fn artifact_abs_path(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<PathBuf, EngineError> {
        let a = find_artifact(&self.data_dir, workspace_id, artifact_id)?;
        artifact_abs_path(&self.data_dir, &a)
    }

    /// Moves a PR chat host's artifacts onto `to_id`, rewriting metadata
    /// paths and remapping creator codes. The destination wins on an id
    /// collision so a live workspace is not overwritten.
    pub fn hoist_workspace_artifacts(
        &self,
        from_id: &str,
        to_id: &str,
        remap: &HashMap<String, String>,
    ) -> Result<(), EngineError> {
        let (from_id, to_id) = (from_id.trim(), to_id.trim());
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return Ok(());
        }
        let list = list_artifacts(&self.data_dir, from_id)?;
        if list.is_empty() {
            return Ok(());
        }
        let dest_dir = recur_store::paths::artifacts_dir(&self.data_dir, to_id).map_err(io_err)?;
        let src_dir = self
            .data_dir
            .join("workspaces")
            .join(from_id)
            .join("artifacts");
        for mut a in list {
            let mut filename = match a.path.rfind('/') {
                Some(i) => a.path[i + 1..].to_owned(),
                None => a.path.clone(),
            };
            if filename.is_empty() {
                let Some(ext) = artifact_ext(&a.kind) else {
                    continue;
                };
                filename = format!("{}{ext}", a.id);
            }
            let dest_file = dest_dir.join(&filename);
            let dest_meta = dest_dir.join(format!("{}.meta.json", a.id));
            if dest_meta.exists() || dest_file.exists() {
                continue;
            }
            let src_file = src_dir.join(&filename);
            move_artifact_file(&src_file, &dest_file).map_err(|e| {
                EngineError(format!(
                    "hoist artifact {}: {}",
                    a.id,
                    hoist_move_err(&src_file, &dest_file, &e)
                ))
            })?;
            a.workspace_id = to_id.to_owned();
            a.path = format!("workspaces/{to_id}/artifacts/{filename}");
            if let Some(next) = remap.get(&a.created_by).filter(|n| !n.is_empty()) {
                a.created_by = next.clone();
            }
            let src_meta = src_dir.join(format!("{}.meta.json", a.id));
            commit_hoisted_artifact(&src_file, &dest_file, &src_meta, &dest_meta, &a)
                .map_err(|e| EngineError(format!("hoist artifact {}: {e}", a.id)))?;
            self.publish_artifact_created(to_id, &a);
        }
        Ok(())
    }
}

/// The error text for a failed fallback copy (the read of the source).
fn hoist_move_err(src: &Path, _dst: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", src.to_string_lossy())
}

#[cfg(test)]
mod tests;
