//! Chat image attachments on disk: pasted images are validated and written
//! under `<data>/workspaces/<id>/attachments/<id>.<ext>`, and reloaded for
//! Retry, loop goals and harness history. Marker grammar and content blocks
//! live in `recur_agent::attach`.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use recur_agent::UserImage;
use recur_agent::attach::{
    CHAT_IMAGE_MIME, MAX_CHAT_ATTACHMENT_BYTES, chat_image_ext, marker, sanitize_attachment_id,
};
use recur_domain::{ChatAttachment, ImagePayload};

use crate::EngineError;

/// The data-dir-relative path of a stored attachment.
fn rel_path(workspace_id: &str, filename: &str) -> String {
    format!("workspaces/{workspace_id}/attachments/{filename}")
}

/// Validates pasted images, writes them to the workspace's attachments dir,
/// and returns the prompt (trimmed, with a marker appended for any image the
/// UI did not mark) plus the images for the turn and the mirror. Unknown
/// types, undecodable data and duplicate ids are skipped; an oversized image
/// fails the send.
pub fn prepare_chat_attachments(
    data_dir: &Path,
    workspace_id: &str,
    text: &str,
    atts: &[ChatAttachment],
) -> Result<(String, Vec<UserImage>), EngineError> {
    let mut text = text.trim().to_owned();
    if atts.is_empty() {
        return Ok((text, Vec::new()));
    }
    let dir = recur_store::paths::attachments_dir(data_dir, workspace_id)
        .map_err(|e| EngineError(e.to_string()))?;
    let mut out: Vec<UserImage> = Vec::with_capacity(atts.len());
    for a in atts {
        let id = sanitize_attachment_id(&a.id);
        if id.is_empty() || out.iter().any(|i| i.id == id) {
            continue;
        }
        let mut mime = a.mime.trim().to_lowercase();
        let Some(ext) = chat_image_ext(&mime) else {
            continue;
        };
        if mime == "image/jpg" {
            mime = "image/jpeg".into();
        }
        let raw = match crate::text::go_base64_decode(a.data.trim()) {
            Ok(raw) if !raw.is_empty() => raw,
            _ => continue,
        };
        if raw.len() > MAX_CHAT_ATTACHMENT_BYTES {
            return Err(EngineError(format!(
                "attachment {id} too large ({} bytes)",
                raw.len()
            )));
        }
        let mut name = a.name.trim().to_owned();
        if name.is_empty() {
            name = format!("{id}{ext}");
        }
        let filename = format!("{id}{ext}");
        std::fs::write(dir.join(&filename), &raw).map_err(|e| EngineError(e.to_string()))?;
        let m = marker(&id);
        if !text.contains(&m) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&m);
        }
        out.push(UserImage {
            id,
            name,
            mime,
            data: STANDARD.encode(&raw),
            path: rel_path(workspace_id, &filename),
        });
    }
    Ok((text, out))
}

/// Reloads a persisted attachment from its data-dir path, so a Retry sends
/// the same bytes. `None` when the file is gone, empty or oversized.
pub fn user_image_from_stored(data_dir: &Path, p: &ImagePayload) -> Option<UserImage> {
    if p.id.is_empty() || p.path.is_empty() {
        return None;
    }
    let raw = std::fs::read(data_dir.join(p.path.trim_start_matches('/'))).ok()?;
    if raw.is_empty() || raw.len() > MAX_CHAT_ATTACHMENT_BYTES {
        return None;
    }
    Some(UserImage {
        id: p.id.clone(),
        name: p.name.clone(),
        mime: p.mime.clone(),
        data: STANDARD.encode(&raw),
        path: p.path.clone(),
    })
}

/// The stored paste `id` of a workspace, whatever its extension.
pub fn user_image_by_id(data_dir: &Path, workspace_id: &str, id: &str) -> Option<UserImage> {
    let workspace_id = workspace_id.trim();
    let id = sanitize_attachment_id(id);
    if workspace_id.is_empty() || id.is_empty() {
        return None;
    }
    let dir = recur_store::paths::attachments_dir(data_dir, workspace_id).ok()?;
    let mut seen_ext: Vec<&str> = Vec::new();
    for (mime, ext) in CHAT_IMAGE_MIME {
        if seen_ext.contains(ext) {
            continue;
        }
        seen_ext.push(ext);
        let filename = format!("{id}{ext}");
        if !dir.join(&filename).exists() {
            continue;
        }
        let mime = if *mime == "image/jpg" {
            "image/jpeg"
        } else {
            mime
        };
        return user_image_from_stored(
            data_dir,
            &ImagePayload {
                id: id.clone(),
                name: filename.clone(),
                mime: mime.to_owned(),
                path: rel_path(workspace_id, &filename),
            },
        );
    }
    None
}

/// The given attachment ids reloaded from the workspace's paste dir, in
/// order; missing ones are skipped.
pub fn user_images_by_ids(data_dir: &Path, workspace_id: &str, ids: &[String]) -> Vec<UserImage> {
    ids.iter()
        .filter_map(|id| user_image_by_id(data_dir, workspace_id, id))
        .collect()
}

/// Every saved chat paste of a workspace.
pub fn workspace_user_images(data_dir: &Path, workspace_id: &str) -> Vec<UserImage> {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Vec::new();
    }
    let Ok(dir) = recur_store::paths::attachments_dir(data_dir, workspace_id) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    // Entries sorted by name.
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let ext = match name.rfind('.') {
            Some(i) => name[i..].to_lowercase(),
            None => continue,
        };
        let Some(mime) = recur_agent::attach::chat_image_mime_for_ext(&ext) else {
            continue;
        };
        let stem = &name[..name.len() - ext.len()];
        let id = sanitize_attachment_id(stem);
        if id.is_empty() {
            continue;
        }
        if let Some(img) = user_image_from_stored(
            data_dir,
            &ImagePayload {
                id,
                name: name.clone(),
                mime: mime.to_owned(),
                path: rel_path(workspace_id, &name),
            },
        ) {
            out.push(img);
        }
    }
    out
}

#[cfg(test)]
mod tests;
