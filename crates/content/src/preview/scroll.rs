//! Per-path preview scroll (markdown / SVG overflow, the HTML page's
//! scroll over its message bridge, Mermaid pan and zoom), copied into `fileViews` so it survives
//! session restore. The editor parks its own view; this covers the preview surfaces.

use std::collections::HashMap;

use recur_editor::view_state::{EditorViewMode, FileViewState};

/// A preview's saved offset; `zoom` is Mermaid's camera scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewScroll {
    pub top: f64,
    pub left: f64,
    pub zoom: Option<f64>,
}

impl PreviewScroll {
    pub fn at(top: f64, left: f64) -> PreviewScroll {
        PreviewScroll {
            top,
            left,
            zoom: None,
        }
    }
}

/// A Mermaid camera to restore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub top: f64,
    pub left: f64,
    pub zoom: f64,
}

/// `previewCamera`: a saved camera, or `None` for the untouched origin.
pub fn preview_camera(s: Option<PreviewScroll>) -> Option<Camera> {
    let s = s?;
    let zoom = s.zoom.filter(|z| *z > 0.).unwrap_or(0.);
    if s.top == 0. && s.left == 0. && zoom == 0. {
        return None;
    }
    Some(Camera {
        top: s.top,
        left: s.left,
        zoom: if zoom == 0. { 1. } else { zoom },
    })
}

/// A scrollable preview surface.
pub trait Scroller {
    fn top(&self) -> f64;
    fn left(&self) -> f64;
    /// Set the offset; the surface clamps it to its scroll range.
    fn set(&mut self, top: f64, left: f64);
}

/// `createPreviewScrollStore`: one per mounted owner, so two owners never share offsets.
#[derive(Debug, Default)]
pub struct PreviewScrollStore {
    saved: HashMap<String, PreviewScroll>,
    /// The workspace whose offsets fill `saved`; a teardown after a switch must not write the
    /// outgoing surface into the incoming map.
    bound_workspace: String,
}

fn settled(s: &dyn Scroller, want: PreviewScroll) -> bool {
    (s.top() - want.top).abs() < 1. && (s.left() - want.left).abs() < 1.
}

impl PreviewScrollStore {
    pub fn new() -> PreviewScrollStore {
        PreviewScrollStore::default()
    }

    /// `rememberPreviewScroll`.
    pub fn remember(&mut self, path: &str, pos: PreviewScroll) {
        if path.is_empty() {
            return;
        }
        self.saved.insert(path.to_string(), pos);
    }

    /// `restorePreviewScroll`: the saved offset, or the origin for an unseen path.
    pub fn restore(&self, path: &str, el: &mut dyn Scroller) {
        if path.is_empty() {
            return;
        }
        let s = self.saved.get(path);
        el.set(s.map_or(0., |s| s.top), s.map_or(0., |s| s.left));
    }

    /// `peekPreviewScroll`.
    pub fn peek(&self, path: &str) -> Option<PreviewScroll> {
        self.saved.get(path).copied()
    }

    /// `previewScrollFields`: the `previewScroll*` fields to persist for `path`.
    pub fn fields(
        &self,
        path: &str,
        prev: Option<&FileViewState>,
    ) -> (Option<f64>, Option<f64>, Option<f64>) {
        let s = self.saved.get(path);
        let top = s.map_or_else(
            || prev.and_then(|p| p.preview_scroll_top).unwrap_or(0.),
            |s| s.top,
        );
        let left = s.map_or_else(
            || prev.and_then(|p| p.preview_scroll_left).unwrap_or(0.),
            |s| s.left,
        );
        let zoom = s.map_or_else(
            || prev.and_then(|p| p.preview_zoom).unwrap_or(0.),
            |s| s.zoom.unwrap_or(0.),
        );
        (
            (top != 0.).then_some(top),
            (left != 0.).then_some(left),
            (zoom > 0.).then_some(zoom),
        )
    }

    /// `hydratePreviewScroll`: replace the map from persisted views, bound to `workspace`.
    pub fn hydrate<'a>(
        &mut self,
        views: impl IntoIterator<Item = (&'a String, &'a FileViewState)>,
        workspace: &str,
    ) {
        self.saved.clear();
        self.bound_workspace = workspace.to_string();
        for (path, vs) in views {
            let top = vs.preview_scroll_top.unwrap_or(0.);
            let left = vs.preview_scroll_left.unwrap_or(0.);
            let zoom = vs.preview_zoom;
            if top != 0. || left != 0. || zoom.is_some_and(|z| z > 0.) {
                self.saved
                    .insert(path.clone(), PreviewScroll { top, left, zoom });
            }
        }
    }

    /// `applyPreviewScrollToFileViews`: `views` with live offsets for `keep`, or `None` when
    /// nothing changed.
    pub fn apply_to_file_views<'a>(
        &self,
        views: &HashMap<String, FileViewState>,
        keep: impl IntoIterator<Item = &'a str>,
    ) -> Option<HashMap<String, FileViewState>> {
        let mut out: Option<HashMap<String, FileViewState>> = None;
        for path in keep {
            if !self.saved.contains_key(path) {
                continue;
            }
            let prev = views.get(path);
            let mut next = prev.cloned().unwrap_or(FileViewState {
                view_mode: EditorViewMode::Preview,
                line: 1,
                col: 1,
                ..Default::default()
            });
            let (t, l, z) = self.fields(path, prev);
            next.preview_scroll_top = t;
            next.preview_scroll_left = l;
            next.preview_zoom = z;
            if let Some(p) = prev
                && p.preview_scroll_top == t
                && p.preview_scroll_left == l
                && p.preview_zoom == z
            {
                continue;
            }
            out.get_or_insert_with(|| views.clone())
                .insert(path.to_string(), next);
        }
        out
    }

    /// `canSave`: not across a workspace switch, and not while parked (reparenting zeroes
    /// overflow).
    pub fn can_save(&self, workspace: &str, parked: bool) -> bool {
        if !workspace.is_empty()
            && !self.bound_workspace.is_empty()
            && workspace != self.bound_workspace
        {
            return false;
        }
        !parked
    }
}

/// `attachPreviewScroll` on a scrolling element: restore on attach, keep retrying while
/// content grows, save the user's scrolling (but never a restore-induced clamp), save on detach.
#[derive(Debug)]
pub struct Attachment {
    path: String,
    workspace: String,
    pending: bool,
    applied: (f64, f64),
    /// `data-preview-top/left`: the offset hosts stash with hot-shell geometry.
    pub marked: (f64, f64),
}

impl Attachment {
    /// `None` when there is no path or the loaded document is not this path yet.
    pub fn attach(
        store: &PreviewScrollStore,
        path: &str,
        loaded: &str,
        workspace: &str,
        el: &mut dyn Scroller,
    ) -> Option<Attachment> {
        if path.is_empty() || loaded != path {
            return None;
        }
        let want = store.peek(path);
        let mut a = Attachment {
            path: path.to_string(),
            workspace: workspace.to_string(),
            pending: want.is_some_and(|w| w.top != 0. || w.left != 0.),
            applied: (0., 0.),
            marked: (0., 0.),
        };
        a.apply_restore(store, el);
        Some(a)
    }

    fn apply_restore(&mut self, store: &PreviewScrollStore, el: &mut dyn Scroller) {
        store.restore(&self.path, el);
        let s = store.peek(&self.path);
        self.marked = (s.map_or(0., |s| s.top), s.map_or(0., |s| s.left));
        self.applied = (el.top(), el.left());
        if s.is_none_or(|s| settled(el, s)) {
            self.pending = false;
        }
    }

    /// The next frame or a resize: retry a restore the content could not hold yet.
    pub fn try_restore(&mut self, store: &PreviewScrollStore, el: &mut dyn Scroller) {
        if self.pending {
            self.apply_restore(store, el);
        }
    }

    /// A scroll (or the detach): remember the user's offset.
    pub fn save(&mut self, store: &mut PreviewScrollStore, el: &dyn Scroller, parked: bool) {
        if !store.can_save(&self.workspace, parked) {
            return;
        }
        if self.pending
            && (el.top() - self.applied.0).abs() < 1.
            && (el.left() - self.applied.1).abs() < 1.
        {
            return;
        }
        self.pending = false;
        store.remember(&self.path, PreviewScroll::at(el.top(), el.left()));
        self.marked = (el.top(), el.left());
    }

    /// Teardown: one last save.
    pub fn detach(mut self, store: &mut PreviewScrollStore, el: &dyn Scroller, parked: bool) {
        self.save(store, el, parked);
    }
}

/// `attachIframeScroll`: the HTML page reports its scroll over messages; the host restores by
/// message on attach and on every load.
#[derive(Debug)]
pub struct PageAttachment {
    path: String,
    workspace: String,
    seen_live: bool,
    pub marked: (f64, f64),
}

impl PageAttachment {
    /// Returns the attachment and the restore offset to send the page.
    pub fn attach(
        store: &PreviewScrollStore,
        path: &str,
        workspace: &str,
    ) -> (PageAttachment, (f64, f64)) {
        let want = store.peek(path);
        let mut a = PageAttachment {
            path: path.to_string(),
            workspace: workspace.to_string(),
            // The page paints at 0 before the restore lands; that 0 must not replace the
            // offset still being applied.
            seen_live: !want.is_some_and(|w| w.top != 0. || w.left != 0.),
            marked: (0., 0.),
        };
        let at = a.restore(store);
        (a, at)
    }

    /// The restore message (on attach and on load).
    pub fn restore(&mut self, store: &PreviewScrollStore) -> (f64, f64) {
        let s = store.peek(&self.path);
        let at = (s.map_or(0., |s| s.top), s.map_or(0., |s| s.left));
        self.marked = at;
        at
    }

    /// A `recur-preview-scroll` message from the page.
    pub fn on_scroll(&mut self, store: &mut PreviewScrollStore, top: f64, left: f64, parked: bool) {
        if !store.can_save(&self.workspace, parked) {
            return;
        }
        if !self.seen_live && top == 0. && left == 0. {
            return;
        }
        self.seen_live = true;
        store.remember(&self.path, PreviewScroll::at(top, left));
        self.marked = (top, left);
    }
}

#[cfg(test)]
mod tests;
