//! Worktree and external files through the asset routes (`lib/workspace-file.ts`): the URLs
//! the Svelte UI fetched, served natively by `AssetsService::fetch_asset`.

use recur_domain::FileContent;
use recur_services::Services;

fn query(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

/// `workspaceFileURL`: a worktree file without the JSON bridge. `mtime` 0 is omitted; `base`
/// asks for the merge-base revision.
pub fn workspace_file_url(
    project_id: &str,
    workspace_id: &str,
    path: &str,
    mtime: i64,
    base: bool,
) -> String {
    let t = mtime.to_string();
    let mut pairs = vec![("project", project_id)];
    if !workspace_id.is_empty() {
        pairs.push(("workspace", workspace_id));
    }
    pairs.push(("path", path));
    if mtime != 0 {
        pairs.push(("t", &t));
    }
    if base {
        pairs.push(("at", "base"));
    }
    format!("/workspace-file?{}", query(&pairs))
}

pub const EXTERNAL_FILE_PATH: &str = "/external-file";

/// `externalFileURL`: an absolute-path file outside the worktree.
pub fn external_file_url(abs_path: &str, mtime: i64) -> String {
    let t = mtime.to_string();
    let mut pairs = vec![("path", abs_path)];
    if mtime != 0 {
        pairs.push(("t", &t));
    }
    format!("{EXTERNAL_FILE_PATH}?{}", query(&pairs))
}

/// `isBinaryPreviewMime`: raster, PDF or video streams, never editable text.
pub fn is_binary_preview_mime(mime: &str) -> bool {
    !mime.is_empty()
        && (mime.starts_with("image/") || mime.starts_with("video/") || mime == "application/pdf")
}

/// `fileNeedsHTTPBody`: ReadFile returned metadata only.
pub fn file_needs_http_body(f: &FileContent) -> bool {
    if !f.mime.is_empty() {
        return false;
    }
    f.content.is_empty() && f.size > 0
}

/// `fetchWorkspaceFileText`.
pub async fn fetch_workspace_file_text(
    services: &Services,
    project_id: &str,
    workspace_id: &str,
    path: &str,
) -> Result<String, String> {
    let url = workspace_file_url(project_id, workspace_id, path, 0, false);
    match services.assets.fetch_asset(&url).await {
        Ok(a) => Ok(String::from_utf8_lossy(&a.body).into_owned()),
        Err(e) => {
            let msg = e.message.trim().to_string();
            Err(if msg.is_empty() {
                format!("HTTP {}", e.status)
            } else {
                msg
            })
        }
    }
}

/// The raw bytes behind a URL (images, PDFs, videos, artifacts).
pub async fn fetch_bytes(services: &Services, url: &str) -> Result<(String, Vec<u8>), String> {
    services
        .assets
        .fetch_asset(url)
        .await
        .map(|a| (a.content_type, a.body))
        .map_err(|e| {
            let msg = e.message.trim().to_string();
            if msg.is_empty() {
                format!("not found ({})", e.status)
            } else {
                msg
            }
        })
}

/// `readExternalEditorFile`: an absolute-path file as a read-only editor file.
pub async fn read_external_editor_file(
    services: &Services,
    path: &str,
) -> Result<FileContent, String> {
    let url = external_file_url(path, 0);
    let asset = services.assets.fetch_asset(&url).await.map_err(|e| {
        let msg = e.message.trim().to_string();
        if msg.is_empty() {
            format!("not found: {path}")
        } else {
            msg
        }
    })?;
    let raw_type = asset
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if is_binary_preview_mime(&raw_type) {
        return Ok(FileContent {
            path: path.into(),
            language: String::new(),
            content: String::new(),
            size: asset.body.len() as i64,
            writable: false,
            mime: raw_type,
            mtime: 0,
        });
    }
    let content = String::from_utf8_lossy(&asset.body).into_owned();
    let norm = path.replace('\\', "/");
    let ext = norm.rfind('.').map(|i| &path[i + 1..]).unwrap_or("");
    Ok(FileContent {
        path: path.into(),
        language: if ext.is_empty() {
            "text".into()
        } else {
            ext.into()
        },
        size: content.encode_utf16().count() as i64,
        content,
        writable: false,
        mime: String::new(),
        mtime: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_file_url_encodes_project_path_and_base() {
        assert_eq!(
            workspace_file_url("p1", "w1", "assets/icon.png", 12, false),
            "/workspace-file?project=p1&workspace=w1&path=assets%2Ficon.png&t=12"
        );
        assert_eq!(
            workspace_file_url("p1", "w1", "assets/icon.png", 0, true),
            "/workspace-file?project=p1&workspace=w1&path=assets%2Ficon.png&at=base"
        );
    }

    #[test]
    fn external_file_url_encodes_the_absolute_path() {
        assert_eq!(
            external_file_url("/Users/josh/Downloads/export-111.csv", 0),
            "/external-file?path=%2FUsers%2Fjosh%2FDownloads%2Fexport-111.csv"
        );
    }

    #[test]
    fn file_needs_http_body_only_without_mime_and_content() {
        let f = |mime: &str, content: &str, size: i64| FileContent {
            mime: mime.into(),
            content: content.into(),
            size,
            ..Default::default()
        };
        assert!(!file_needs_http_body(&f("application/pdf", "", 1200)));
        assert!(!file_needs_http_body(&f("image/png", "", 80)));
        assert!(file_needs_http_body(&f("", "", 80)));
        assert!(!file_needs_http_body(&f("", "hi", 2)));
    }

    #[test]
    fn binary_preview_mimes() {
        assert!(is_binary_preview_mime("image/png"));
        assert!(is_binary_preview_mime("video/mp4"));
        assert!(is_binary_preview_mime("application/pdf"));
        assert!(!is_binary_preview_mime("text/plain"));
        assert!(!is_binary_preview_mime(""));
    }
}
