//! The main pane's media surfaces (`.obs-image`, `.obs-pdf`, the video pane): the current and
//! base rasters (`imageSrc`, `imageBaseSrc`, the side-by-side compare in Diff), the PDF bytes,
//! the zoom lightbox, and the host's image loader over the asset routes.

use std::sync::Arc;

use futures::future::BoxFuture;
use gpui_kit::{App, AppContext as _, Context, Entity, Window};
use recur_content::images::{Decoded, ImageCache, ImageLoader};
use recur_content::{AttachmentLightbox, FilePreviewEvent, PdfPreview};
use recur_editor::view_state::{EditorViewMode, is_binary_image_path};
use recur_services::Services;
use recur_ui::guard::Guarded;
use recur_view_core::file_drag::is_abs_filesystem_path;

use crate::model::is_scratch_path;
use crate::obs::Observatory;
use crate::workspace_file::{external_file_url, workspace_file_url};

/// Image bytes through `AssetsService` (`/workspace-file`, `/external-file`, `/gh-image`, …).
pub struct ServicesImageLoader(pub Services);

impl ImageLoader for ServicesImageLoader {
    fn load(&self, src: &str) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let svc = self.0.clone();
        let src = src.to_string();
        Box::pin(async move {
            svc.assets
                .fetch_asset(&src)
                .await
                .map(|a| a.body)
                .map_err(|e| e.message)
        })
    }
}

/// Install the asset-route image loader unless the host already has one.
pub fn install_image_loader(services: &Services, cx: &mut App) {
    if cx.has_global::<recur_content::images::Loader>() {
        return;
    }
    recur_content::set_image_loader(cx, Arc::new(ServicesImageLoader(services.clone())));
}

/// A device size and the raster resampled to it.
type Scaled = ((u32, u32), std::sync::Arc<gpui_kit::RenderImage>);

/// A raster loading or loaded for a `src`.
#[derive(Clone, Default)]
pub struct Raster {
    pub src: String,
    pub image: Option<Decoded>,
    pub failed: bool,
    /// The raster resampled to its painted device size.
    pub scaled: std::cell::RefCell<Option<Scaled>>,
}

impl Raster {
    /// The image to paint at `w`×`h` device pixels.
    pub fn at(&self, w: u32, h: u32) -> Option<std::sync::Arc<gpui_kit::RenderImage>> {
        let d = self.image.as_ref()?;
        if (d.width as u32, d.height as u32) == (w, h) {
            return Some(d.image.clone());
        }
        let mut c = self.scaled.borrow_mut();
        if let Some((size, img)) = c.as_ref()
            && *size == (w, h)
        {
            return Some(img.clone());
        }
        let img = recur_content::images::resized(d, w, h)?;
        *c = Some(((w, h), img.clone()));
        Some(img)
    }
}

/// The media pane's state (`imageBaseFailedSrc`, the decoded images, the PDF document).
#[derive(Default)]
pub struct MediaPane {
    pub current: Raster,
    pub base: Raster,
    /// `imageBaseFailedSrc`: the base revision does not exist (the image was added).
    pub base_failed_src: String,
    pub pdf_src: String,
}

impl Observatory {
    /// `imageSrc`, or the media pane's `mediaSrc` (the same URL rules).
    pub(crate) fn file_src(&self) -> String {
        let Some(f) = &self.file else {
            return String::new();
        };
        if is_scratch_path(&f.path) && !f.content.is_empty() {
            return format!("data:{};base64,{}", f.mime, f.content);
        }
        if is_abs_filesystem_path(&f.path) {
            return external_file_url(&f.path, f.mtime);
        }
        workspace_file_url(
            &self.props.project_id,
            &self.props.workspace_id,
            &f.path,
            f.mtime,
            false,
        )
    }

    pub(crate) fn image_src(&self) -> String {
        if !self.is_image() {
            return String::new();
        }
        self.file_src()
    }

    pub(crate) fn media_src(&self) -> String {
        if !self.is_pdf() && !self.is_video() {
            return String::new();
        }
        self.file_src()
    }

    /// `imageBaseSrc`: the previous revision of a modified or deleted raster.
    pub(crate) fn image_base_src(&self) -> String {
        let p = &self.selected_path;
        if p.is_empty() || is_scratch_path(p) || is_abs_filesystem_path(p) {
            return String::new();
        }
        let st = self.selected_git_status();
        if st != "m" && st != "d" {
            return String::new();
        }
        if !self.is_image() && !is_binary_image_path(p) {
            return String::new();
        }
        workspace_file_url(
            &self.props.project_id,
            &self.props.workspace_id,
            p,
            self.base_doc_epoch as i64 + 1,
            true,
        )
    }

    /// `imageCompare`: Diff shows previous and current side by side.
    pub(crate) fn image_compare(&self) -> bool {
        let base = self.image_base_src();
        self.view_mode == EditorViewMode::Diff
            && !base.is_empty()
            && self.image.base_failed_src != base
            && (self.selected_git_status() == "d"
                || self
                    .file
                    .as_ref()
                    .is_some_and(|f| f.path == self.selected_path))
    }

    fn load_raster(&mut self, base: bool, src: String, cx: &mut Context<Self>) {
        let slot = if base {
            &mut self.image.base
        } else {
            &mut self.image.current
        };
        if slot.src == src {
            return;
        }
        *slot = Raster {
            src: src.clone(),
            ..Default::default()
        };
        if src.is_empty() {
            return;
        }
        if let Some(d) = cx
            .try_global::<ImageCache>()
            .and_then(|c| c.0.get(&src))
            .cloned()
        {
            let slot = if base {
                &mut self.image.base
            } else {
                &mut self.image.current
            };
            slot.image = Some(d);
            return;
        }
        let Some(fut) = recur_content::images::fetch(&src, cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(fut).await;
            let _ = this.update(cx, |o, cx| {
                let slot = if base {
                    &mut o.image.base
                } else {
                    &mut o.image.current
                };
                if slot.src != src {
                    return;
                }
                match res {
                    Ok(d) => {
                        cx.default_global::<ImageCache>()
                            .0
                            .insert(src.clone(), d.clone());
                        let slot = if base {
                            &mut o.image.base
                        } else {
                            &mut o.image.current
                        };
                        slot.image = Some(d);
                    }
                    Err(_) => {
                        slot.failed = true;
                        if base {
                            o.image.base_failed_src = src.clone();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the decoded rasters and the PDF document in step with the open file.
    pub(crate) fn sync_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_image_pane() {
            let cur = if self.file.is_some() && self.selected_git_status() != "d" {
                self.image_src()
            } else {
                String::new()
            };
            self.load_raster(false, cur, cx);
            let base = if self.image_compare() {
                self.image_base_src()
            } else {
                String::new()
            };
            self.load_raster(true, base, cx);
        }
        let pdf_src = if self.show_pdf_pane() && self.file.is_some() {
            self.media_src()
        } else {
            String::new()
        };
        if pdf_src.is_empty() {
            self.pdf_preview = None;
            self.image.pdf_src.clear();
            return;
        }
        if self.image.pdf_src == pdf_src && self.pdf_preview.is_some() {
            return;
        }
        self.image.pdf_src = pdf_src.clone();
        let pdf = match &self.pdf_preview {
            Some(p) => p.clone(),
            None => {
                let p = cx.new(|cx| PdfPreview::new(None, window, cx));
                let sub =
                    cx.guarded_subscribe_in(&p, window, |obs, _, _: &FilePreviewEvent, _, cx| {
                        obs.sync_find_panel_open(cx)
                    });
                self.editor_subs.push(sub);
                self.pdf_preview = Some(p.clone());
                p
            }
        };
        if let Some(bytes) = recur_content::images::data_url_bytes(&pdf_src) {
            let data: Arc<[u8]> = bytes.into();
            pdf.update(cx, |p, cx| p.set_data(Some(data), window, cx));
            return;
        }
        let svc = self.services.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = crate::workspace_file::fetch_bytes(&svc, &pdf_src).await;
            let _ = this.update_in(cx, |o, window, cx| {
                if o.image.pdf_src != pdf_src {
                    return;
                }
                let data: Option<Arc<[u8]>> = res.ok().map(|(_, b)| b.into());
                pdf.update(cx, |p, cx| p.set_data(data, window, cx));
            });
        })
        .detach();
    }

    /// Open the zoom lightbox on an image (`mediaZoom`).
    pub(crate) fn zoom_image(
        &mut self,
        src: String,
        name: String,
        base: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let image = if base {
            self.image.base.image.clone()
        } else {
            self.image.current.image.clone()
        };
        let weak = self.weak.clone();
        let on_close = std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
            if let Some(o) = weak.upgrade() {
                o.update(cx, |o, cx| {
                    o.media_zoom = None;
                    cx.notify();
                });
            }
        });
        let lb = cx.new(|cx| {
            AttachmentLightbox::new(
                src.clone(),
                name.clone(),
                image.clone(),
                on_close,
                window,
                cx,
            )
        });
        self.media_zoom = Some(lb);
        cx.notify();
    }

    /// The video pane's zoom: GPUI has no media element, so the file opens in the system
    /// player.
    /// `paneVideo`: the player for the selected video, kept while the same file shows.
    pub(crate) fn video_player(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Entity<recur_content::video::VideoPlayer>> {
        let f = self.file.as_ref()?;
        let abs = if is_abs_filesystem_path(&f.path) {
            std::path::PathBuf::from(&f.path)
        } else {
            std::path::Path::new(&self.workspace_root(cx)).join(&f.path)
        };
        if let Some(v) = &self.video
            && v.read(cx).path() == abs
        {
            return Some(v.clone());
        }
        let v = cx.new(|cx| {
            recur_content::video::VideoPlayer::new(
                abs,
                recur_content::video::VideoOptions::default(),
                cx,
            )
        });
        self.video = Some(v.clone());
        Some(v)
    }

    /// `zoomVideo`: pause the pane and play on in the lightbox from the same time; closing it
    /// puts the pane at the lightbox's time.
    pub(crate) fn zoom_video(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(f), Some(v)) = (self.file.clone(), self.video.clone()) else {
            return;
        };
        let (path, start) = v.update(cx, |p, cx| {
            p.pause(cx);
            (p.path().to_path_buf(), p.position())
        });
        let weak = self.weak.clone();
        let on_close = std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
            let Some(o) = weak.upgrade() else { return };
            let lb = o.update(cx, |o, cx| {
                cx.notify();
                o.media_zoom.take()
            });
            // The lightbox is mid-update while it closes: read its time once that ends.
            let Some(lb) = lb else { return };
            cx.defer(move |cx| {
                let t = lb.read(cx).video_position(cx);
                let v = o.read(cx).video.clone();
                if let (Some(t), Some(v)) = (t, v) {
                    v.update(cx, |p, cx| p.seek(t, cx));
                }
            });
        });
        let lb = cx
            .new(|cx| AttachmentLightbox::video(path, f.path.clone(), start, on_close, window, cx));
        self.media_zoom = Some(lb);
        cx.notify();
    }
}
