//! The asset routes `main.go` mounted as Wails asset middleware (Go
//! `fileserver.go`, `serveWorkspaceData`, the `/gh-image` handler), served
//! through [`AssetsService`] for a UI that has no webview. Statuses and
//! error text match the HTTP handlers; ranges and `HEAD` have no native
//! caller and are not served.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use image::ImageDecoder as _;

use super::AppServices;
use crate::api::{Asset, AssetError, AssetsService};

/// Whether TIFFs are served as PNG. Go transcoded where the webview could
/// not decode TIFF (everything but WKWebView); the native image decoder
/// cannot either, so it is on everywhere. Tests turn it off.
static TRANSCODE_TIFF: AtomicBool = AtomicBool::new(true);

/// A small compressed TIFF must not balloon into gigabytes of RGBA.
const MAX_TIFF_PIXELS: u64 = 1 << 26;

#[cfg(test)]
pub(crate) fn set_transcode_tiff(on: bool) -> bool {
    TRANSCODE_TIFF.swap(on, Ordering::SeqCst)
}

type Served = std::result::Result<Asset, AssetError>;

fn no_cache(content_type: impl Into<String>, body: Vec<u8>) -> Asset {
    Asset {
        content_type: content_type.into(),
        cache_control: "no-cache".into(),
        body,
    }
}

/// Go `writeTIFFAsPNG`: the file is not modified.
fn tiff_as_png(data: &[u8]) -> Served {
    let fail = |e: String| AssetError::new(415, e);
    let dec = image::codecs::tiff::TiffDecoder::new(Cursor::new(data))
        .map_err(|e| fail(e.to_string()))?;
    let (w, h) = dec.dimensions();
    if h > 0 && u64::from(w) > MAX_TIFF_PIXELS / u64::from(h) {
        return Err(fail(format!("image too large ({w}x{h})")));
    }
    let img = image::DynamicImage::from_decoder(dec).map_err(|e| fail(e.to_string()))?;
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| fail(e.to_string()))?;
    Ok(no_cache("image/png", png))
}

fn status_for(e: &recur_code::Error, default: u16) -> u16 {
    let msg = e.to_string();
    if matches!(e, recur_code::Error::NotFound(_)) {
        404
    } else if msg.contains("too large") {
        413
    } else if msg.contains("not a chat image") {
        415
    } else if msg.contains("path not allowed") {
        403
    } else {
        default
    }
}

/// Reads a prepared file and types it: TIFF transcoded, other images by
/// mime, and text only when it is not binary.
fn serve_prepared(abs: &Path, mime: &str) -> Served {
    let mut f = match std::fs::File::open(abs) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AssetError::not_found()),
        Err(e) => return Err(AssetError::new(500, e.to_string())),
    };
    if mime.is_empty()
        && let Err(e) = recur_code::rewind_binary_check(&mut f)
    {
        return Err(AssetError::new(415, e.to_string()));
    }
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut body)
        .map_err(|e| AssetError::new(500, e.to_string()))?;
    if mime == "image/tiff" && TRANSCODE_TIFF.load(Ordering::SeqCst) {
        return tiff_as_png(&body);
    }
    if mime.is_empty() {
        return Ok(no_cache("text/plain; charset=utf-8", body));
    }
    Ok(no_cache(mime, body))
}

/// Go `mime.TypeByExtension` for the files workspaces store (attachments,
/// screenshots, artifacts), else a content sniff.
fn data_content_type(path: &Path, body: &[u8]) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let by_ext = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "txt" | "log" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.into();
    }
    match image::guess_format(body) {
        Ok(f) => f.to_mime_type().into(),
        Err(_) if std::str::from_utf8(body).is_ok() => "text/plain; charset=utf-8".into(),
        Err(_) => "application/octet-stream".into(),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl AppServices {
    /// `/workspaces/<id>/…` under `<data>/workspaces`, refusing anything that
    /// escapes it.
    fn serve_workspace_data(&self, rel: &str) -> Served {
        let base = self.data_dir().join("workspaces");
        let cleaned = recur_bundles::go::clean(&format!("/{rel}"));
        let full: PathBuf = base.join(cleaned.trim_start_matches('/'));
        if full != base && !full.starts_with(&base) {
            return Err(AssetError::not_found());
        }
        match std::fs::metadata(&full) {
            Ok(m) if m.is_file() => {}
            _ => return Err(AssetError::not_found()),
        }
        let body = std::fs::read(&full).map_err(|e| AssetError::new(500, e.to_string()))?;
        let ct = data_content_type(&full, &body);
        Ok(Asset {
            content_type: ct,
            cache_control: String::new(),
            body,
        })
    }

    async fn serve_workspace_file(&self, q: &Query) -> Served {
        let (project_id, rel) = (q.get("project"), q.get("path"));
        if project_id.is_empty() || rel.trim().is_empty() {
            return Err(AssetError::new(400, "missing project or path"));
        }
        let at = q.get("at");
        if !at.trim().is_empty() {
            if at.trim() != "base" {
                return Err(AssetError::new(400, "invalid at"));
            }
            return self
                .serve_workspace_image_at_base(project_id, q.get("workspace"), rel)
                .await;
        }
        let (root, rel) = self
            .resolve_read_path(project_id, q.get("workspace"), rel)
            .map_err(|e| {
                let msg = e.to_string();
                let status = if msg.contains("outside the workspace") {
                    403
                } else {
                    404
                };
                AssetError::new(status, msg)
            })?;
        let (abs, mime) = recur_code::prepare_serve(&root, &rel)
            .map_err(|e| AssetError::new(status_for(&e, 400), e.to_string()))?;
        serve_prepared(&abs, mime)
    }

    /// The merge-base (or HEAD) revision of a raster image: files deleted on
    /// disk still exist in git, so this does not go through `prepare_serve`.
    async fn serve_workspace_image_at_base(
        &self,
        project_id: &str,
        workspace_id: &str,
        rel: &str,
    ) -> Served {
        if rel.starts_with('/') {
            return Err(AssetError::new(
                400,
                "base revision is only for workspace paths",
            ));
        }
        let (root, _, base) = self
            .resolve_scope(project_id, workspace_id)
            .map_err(|e| AssetError::new(404, e.to_string()))?;
        let abs =
            recur_code::safe_join(&root, rel).map_err(|e| AssetError::new(400, e.to_string()))?;
        let rel = recur_code::rel_slash(&root, &abs);
        let mime = recur_code::image_mime(&rel);
        if mime.is_empty() {
            return Err(AssetError::new(400, "not an image"));
        }
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = recur_code::image_at_base(&root, &base, &rel, Some(&mut buf)).await {
            return Err(AssetError::new(status_for(&e, 400), e.to_string()));
        }
        if mime == "image/tiff" && TRANSCODE_TIFF.load(Ordering::SeqCst) {
            return tiff_as_png(&buf);
        }
        Ok(no_cache(mime, buf))
    }

    /// Dropped chat images: absolute png/jpeg/gif/webp on the drop
    /// allowlist, 8 MiB cap, magic sniffed.
    fn serve_chat_image(&self, path: &str) -> Served {
        let (abs, mime) = recur_code::prepare_chat_image(path)
            .map_err(|e| AssetError::new(status_for(&e, 400), e.to_string()))?;
        let body = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AssetError::not_found());
            }
            Err(e) => return Err(AssetError::new(500, e.to_string())),
        };
        Ok(no_cache(mime, body))
    }

    /// Dropped files outside every worktree, opened in the editor.
    fn serve_external_file(&self, path: &str) -> Served {
        let (abs, mime) = recur_code::prepare_external(path)
            .map_err(|e| AssetError::new(status_for(&e, 400), e.to_string()))?;
        serve_prepared(&abs, mime)
    }

    /// GitHub attachment images with the user's `gh` token (the webview had
    /// no GitHub cookies; user-attachments 404 without auth).
    async fn serve_forge_image(&self, raw: &str) -> Served {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(AssetError::new(400, "missing url"));
        }
        for forge in self.engine.forges().all() {
            match forge.fetch_image(raw).await {
                Ok(Some(img)) => {
                    return Ok(Asset {
                        content_type: img.content_type,
                        cache_control: "private, max-age=86400".into(),
                        body: img.bytes,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    let status = match e.kind {
                        recur_forge::ForgeErrorKind::Http(n) => n,
                        _ => 502,
                    };
                    return Err(AssetError::new(status, e.message));
                }
            }
        }
        Err(AssetError::new(400, "url not allowed"))
    }

    pub(crate) async fn fetch_asset_at(&self, url: &str) -> Served {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        let path = percent_decode(path);
        let q = Query::parse(query);
        if let Some(rel) = path.strip_prefix("/workspaces/") {
            let me = self.me();
            let rel = rel.to_owned();
            return self
                .off_thread(move || Ok(me.serve_workspace_data(&rel)))
                .await
                .unwrap_or_else(|e| Err(AssetError::new(500, e.message)));
        }
        match path.as_str() {
            "/workspace-file" => self.serve_workspace_file(&q).await,
            "/chat-image" => self.serve_chat_image(q.get("path")),
            "/external-file" => self.serve_external_file(q.get("path")),
            "/gh-image" => self.serve_forge_image(q.get("url")).await,
            _ => Err(AssetError::not_found()),
        }
    }
}

/// Go `url.Values.Get`: the first value of a key, decoded.
struct Query(Vec<(String, String)>);

impl Query {
    fn parse(q: &str) -> Self {
        Self(
            url::form_urlencoded::parse(q.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map_or("", |(_, v)| v.as_str())
    }
}

#[async_trait]
impl AssetsService for AppServices {
    async fn fetch_asset(&self, url: &str) -> Served {
        let s = self.me();
        let url = url.to_owned();
        self.handle
            .spawn(async move { s.fetch_asset_at(&url).await })
            .await
            .unwrap_or_else(|e| Err(AssetError::new(500, e.to_string())))
    }
}
