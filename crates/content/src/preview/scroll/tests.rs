use super::*;

fn vs(top: Option<f64>, left: Option<f64>, zoom: Option<f64>) -> FileViewState {
    FileViewState {
        view_mode: EditorViewMode::Preview,
        line: 1,
        col: 1,
        preview_scroll_top: top,
        preview_scroll_left: left,
        preview_zoom: zoom,
        ..Default::default()
    }
}

/// An element with a scroll range (`boundedScroller`); `None` bounds never clamp.
struct El {
    top: f64,
    left: f64,
    bounds: Option<(f64, f64)>,
}

impl El {
    fn free(top: f64, left: f64) -> El {
        El {
            top,
            left,
            bounds: None,
        }
    }
    fn bounded(max_top: f64, max_left: f64) -> El {
        El {
            top: 0.,
            left: 0.,
            bounds: Some((max_top, max_left)),
        }
    }
}

impl Scroller for El {
    fn top(&self) -> f64 {
        self.top
    }
    fn left(&self) -> f64 {
        self.left
    }
    fn set(&mut self, top: f64, left: f64) {
        match self.bounds {
            Some((mt, ml)) => {
                self.top = top.clamp(0., mt.max(0.));
                self.left = left.clamp(0., ml.max(0.));
            }
            None => {
                self.top = top;
                self.left = left;
            }
        }
    }
}

fn views(entries: &[(&str, FileViewState)]) -> HashMap<String, FileViewState> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn round_trips_a_path_and_resets_an_unseen_one() {
    let mut s = PreviewScrollStore::new();
    s.remember("notes.md", PreviewScroll::at(240., 12.));
    let mut b = El::free(99., 4.);
    s.restore("notes.md", &mut b);
    assert_eq!((b.top, b.left), (240., 12.));
    s.restore("other.md", &mut b);
    assert_eq!((b.top, b.left), (0., 0.));
}

#[test]
fn saves_on_detach_and_restores_on_the_next_attach() {
    let mut s = PreviewScrollStore::new();
    let mut el = El::free(0., 0.);
    let a = Attachment::attach(&s, "readme.md", "readme.md", "", &mut el).unwrap();
    el.top = 180.;
    a.detach(&mut s, &el, false);
    el.top = 0.;
    Attachment::attach(&s, "readme.md", "readme.md", "", &mut el).unwrap();
    assert_eq!(el.top, 180.);
}

#[test]
fn does_not_keep_another_files_offset() {
    let mut s = PreviewScrollStore::new();
    let mut el = El::free(0., 0.);
    let mut a = Attachment::attach(&s, "a.md", "a.md", "", &mut el).unwrap();
    el.top = 300.;
    a.save(&mut s, &el, false);
    a.detach(&mut s, &el, false);
    Attachment::attach(&s, "b.md", "b.md", "", &mut el).unwrap();
    assert_eq!(el.top, 0.);
}

#[test]
fn hydrates_from_file_views_and_copies_back() {
    let mut s = PreviewScrollStore::new();
    let v = views(&[("doc.md", vs(Some(420.), None, None))]);
    s.hydrate(&v, "");
    assert_eq!(s.fields("doc.md", None), (Some(420.), None, None));
    let mut base = vs(None, None, None);
    base.line = 2;
    base.scroll_top = 10.;
    let out = s
        .apply_to_file_views(&views(&[("doc.md", base)]), ["doc.md"])
        .unwrap();
    assert_eq!(out["doc.md"].preview_scroll_top, Some(420.));
    assert_eq!(out["doc.md"].line, 2);
}

#[test]
fn clears_offsets_when_the_live_one_is_zero() {
    let mut s = PreviewScrollStore::new();
    let v = views(&[("doc.md", vs(Some(420.), Some(16.), None))]);
    s.hydrate(&v, "");
    s.remember("doc.md", PreviewScroll::at(0., 0.));
    let out = s.apply_to_file_views(&v, ["doc.md"]).unwrap();
    assert_eq!(out["doc.md"].preview_scroll_top, None);
    assert_eq!(out["doc.md"].preview_scroll_left, None);
}

#[test]
fn restores_once_the_requested_document_has_loaded() {
    let mut s = PreviewScrollStore::new();
    s.remember("long.md", PreviewScroll::at(400., 0.));
    let mut el = El::bounded(0., 0.);
    assert!(Attachment::attach(&s, "long.md", "short.md", "", &mut el).is_none());
    assert_eq!(el.top, 0.);
    assert_eq!(s.peek("long.md"), Some(PreviewScroll::at(400., 0.)));
    el.bounds = Some((720., 0.));
    Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    assert_eq!(el.top, 400.);
}

#[test]
fn does_not_persist_a_clamp_over_a_restore_it_cannot_hold() {
    let mut s = PreviewScrollStore::new();
    s.remember("long.md", PreviewScroll::at(400., 0.));
    let mut el = El::bounded(0., 0.);
    let mut a = Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    assert_eq!(el.top, 0.);
    a.save(&mut s, &el, false);
    a.detach(&mut s, &el, false);
    assert_eq!(s.peek("long.md"), Some(PreviewScroll::at(400., 0.)));
}

#[test]
fn saves_user_scrolling_after_the_range_shrinks() {
    let mut s = PreviewScrollStore::new();
    s.remember("long.md", PreviewScroll::at(400., 0.));
    let mut el = El::bounded(720., 0.);
    let mut a = Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    assert_eq!(el.top, 400.);
    el.bounds = Some((200., 0.));
    el.set(50., 0.);
    a.save(&mut s, &el, false);
    assert_eq!(s.peek("long.md"), Some(PreviewScroll::at(50., 0.)));
    assert_eq!(a.marked.0, 50.);
    a.detach(&mut s, &el, false);
    el.top = 0.;
    Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    assert_eq!(el.top, 50.);
}

#[test]
fn user_scrolling_cancels_a_restore_it_cannot_hold() {
    let mut s = PreviewScrollStore::new();
    s.remember("long.md", PreviewScroll::at(400., 0.));
    let mut el = El::bounded(0., 0.);
    let mut a = Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    el.bounds = Some((200., 0.));
    el.set(50., 0.);
    a.save(&mut s, &el, false);
    assert_eq!(s.peek("long.md"), Some(PreviewScroll::at(50., 0.)));
    assert_eq!(a.marked.0, 50.);
}

#[test]
fn an_unreachable_horizontal_offset_does_not_block_a_vertical_save() {
    let mut s = PreviewScrollStore::new();
    s.remember("wide.md", PreviewScroll::at(400., 300.));
    let mut el = El::bounded(720., 80.);
    let mut a = Attachment::attach(&s, "wide.md", "wide.md", "", &mut el).unwrap();
    assert_eq!((el.top, el.left), (400., 80.));
    el.set(50., 80.);
    a.save(&mut s, &el, false);
    assert_eq!(s.peek("wide.md"), Some(PreviewScroll::at(50., 80.)));
}

#[test]
fn reapplies_the_restore_once_content_can_hold_it() {
    let mut s = PreviewScrollStore::new();
    s.remember("long.md", PreviewScroll::at(400., 0.));
    let mut el = El::bounded(0., 0.);
    let mut a = Attachment::attach(&s, "long.md", "long.md", "", &mut el).unwrap();
    assert_eq!(el.top, 0.);
    el.bounds = Some((720., 0.));
    a.try_restore(&s, &mut el);
    assert_eq!(el.top, 400.);
}

#[test]
fn a_parked_surface_does_not_save() {
    let mut s = PreviewScrollStore::new();
    s.remember("readme.md", PreviewScroll::at(240., 0.));
    let mut el = El::free(240., 0.);
    let a = Attachment::attach(&s, "readme.md", "readme.md", "", &mut el).unwrap();
    el.top = 0.;
    a.detach(&mut s, &el, true);
    assert_eq!(s.peek("readme.md"), Some(PreviewScroll::at(240., 0.)));
}

#[test]
fn a_same_path_workspace_switch_ignores_the_outgoing_teardown() {
    let mut s = PreviewScrollStore::new();
    s.hydrate(&views(&[("README.md", vs(Some(100.), None, None))]), "wsA");
    let mut el = El::free(100., 0.);
    let a = Attachment::attach(&s, "README.md", "README.md", "wsA", &mut el).unwrap();
    s.hydrate(&views(&[("README.md", vs(Some(50.), None, None))]), "wsB");
    a.detach(&mut s, &el, false);
    assert_eq!(
        s.peek("README.md"),
        Some(PreviewScroll {
            top: 50.,
            left: 0.,
            zoom: None
        })
    );
    let mut incoming = El::free(100., 0.);
    Attachment::attach(&s, "README.md", "README.md", "wsB", &mut incoming).unwrap();
    assert_eq!(incoming.top, 50.);
}

#[test]
fn two_owners_keep_separate_maps() {
    let mut a = PreviewScrollStore::new();
    let mut b = PreviewScrollStore::new();
    a.hydrate(&views(&[("README.md", vs(Some(100.), None, None))]), "wsA");
    let mut el = El::free(100., 0.);
    let mut att = Attachment::attach(&a, "README.md", "README.md", "wsA", &mut el).unwrap();
    b.hydrate(&views(&[("README.md", vs(Some(50.), None, None))]), "wsB");
    el.top = 300.;
    att.save(&mut a, &el, false);
    let base = views(&[("README.md", vs(Some(100.), None, None))]);
    let out = a.apply_to_file_views(&base, ["README.md"]).unwrap();
    assert_eq!(out["README.md"].preview_scroll_top, Some(300.));
    assert_eq!(
        b.peek("README.md"),
        Some(PreviewScroll {
            top: 50.,
            left: 0.,
            zoom: None
        })
    );
}

#[test]
fn round_trips_a_mermaid_camera_with_zoom_and_negative_pan() {
    let mut s = PreviewScrollStore::new();
    s.hydrate(
        &views(&[("flow.mmd", vs(Some(40.), Some(12.), Some(2.)))]),
        "",
    );
    assert_eq!(
        preview_camera(s.peek("flow.mmd")),
        Some(Camera {
            top: 40.,
            left: 12.,
            zoom: 2.
        })
    );
    s.remember(
        "flow.mmd",
        PreviewScroll {
            top: 8.,
            left: 3.,
            zoom: Some(1.25),
        },
    );
    let out = s
        .apply_to_file_views(
            &views(&[("flow.mmd", vs(Some(40.), None, Some(2.)))]),
            ["flow.mmd"],
        )
        .unwrap();
    assert_eq!(out["flow.mmd"].preview_scroll_top, Some(8.));
    assert_eq!(out["flow.mmd"].preview_scroll_left, Some(3.));
    assert_eq!(out["flow.mmd"].preview_zoom, Some(1.25));

    let mut s = PreviewScrollStore::new();
    s.remember(
        "flow.mmd",
        PreviewScroll {
            top: -120.,
            left: -80.,
            zoom: Some(2.),
        },
    );
    let out = s
        .apply_to_file_views(&views(&[("flow.mmd", vs(None, None, None))]), ["flow.mmd"])
        .unwrap();
    assert_eq!(out["flow.mmd"].preview_scroll_top, Some(-120.));
    s.hydrate(&out, "");
    assert_eq!(
        preview_camera(s.peek("flow.mmd")),
        Some(Camera {
            top: -120.,
            left: -80.,
            zoom: 2.
        })
    );
}

#[test]
fn restores_the_html_page_by_message() {
    let mut s = PreviewScrollStore::new();
    s.hydrate(&views(&[("a.html", vs(Some(90.), Some(4.), None))]), "");
    let (mut a, at) = PageAttachment::attach(&s, "a.html", "");
    assert_eq!(at, (90., 4.));
    a.on_scroll(&mut s, 120., 8., false);
    assert_eq!(s.peek("a.html"), Some(PreviewScroll::at(120., 8.)));
}

#[test]
fn ignores_the_pages_paint_at_zero_until_a_live_offset() {
    let mut s = PreviewScrollStore::new();
    s.hydrate(&views(&[("a.html", vs(Some(90.), Some(4.), None))]), "");
    let (mut a, _) = PageAttachment::attach(&s, "a.html", "");
    a.on_scroll(&mut s, 0., 0., false);
    assert_eq!(
        s.peek("a.html"),
        Some(PreviewScroll {
            top: 90.,
            left: 4.,
            zoom: None
        })
    );
    a.on_scroll(&mut s, 90., 4., false);
    a.on_scroll(&mut s, 0., 0., false);
    assert_eq!(s.peek("a.html"), Some(PreviewScroll::at(0., 0.)));
}

#[test]
fn switching_html_files_resets_the_marked_offset() {
    let mut s = PreviewScrollStore::new();
    s.hydrate(&views(&[("a.html", vs(Some(90.), Some(4.), None))]), "");
    let (a, _) = PageAttachment::attach(&s, "a.html", "");
    assert_eq!(a.marked, (90., 4.));
    let (b, at) = PageAttachment::attach(&s, "b.html", "");
    assert_eq!(at, (0., 0.));
    assert_eq!(b.marked, (0., 0.));
}
