//! The terminal grid element: fits the grid to its box and paints it the way xterm.js's WebGL
//! renderer does (cell backgrounds, glyphs on a device-pixel grid, underline styles,
//! strikethrough, cursor shapes, link underlines, IME composition and the overlay scrollbar).

use gpui_kit::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler,
    Entity, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString, Style, TextRun,
    Window, fill, point, px, relative, rgba, size,
};
#[cfg(not(target_os = "macos"))]
use recur_term::screen::{CursorShape, Glyph, UnderlineStyle};
use recur_term::screen::{Frame, Rgba};

#[cfg(target_os = "macos")]
use crate::atlas::FontKey;
#[cfg(not(target_os = "macos"))]
use crate::custom_glyphs;
use crate::font::{CellMetrics, mono_font_stack};
#[cfg(target_os = "macos")]
use crate::tiles;
use crate::view::{GridLayout, TerminalView};
use recur_ui::guard;

pub(crate) struct TerminalElement {
    view: Entity<TerminalView>,
}

impl TerminalElement {
    pub(crate) fn new(view: Entity<TerminalView>) -> Self {
        Self { view }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

pub(crate) struct Prepaint {
    layout: GridLayout,
    frame: Frame,
    fonts: Fonts,
    #[cfg(target_os = "macos")]
    atlas_font: FontKey,
    /// None while the pane is hidden: hidden tabs take no pointer events.
    hitboxes: Option<(Hitbox, Hitbox)>,
}

#[derive(Clone)]
pub(crate) struct Fonts {
    regular: Font,
    size: Pixels,
}

impl Fonts {
    #[cfg(not(target_os = "macos"))]
    fn variant(&self, bold: bool, italic: bool) -> Font {
        let mut f = self.regular.clone();
        if bold {
            f.weight = FontWeight::BOLD;
        }
        if italic {
            f.style = FontStyle::Italic;
        }
        f
    }
}

/// A terminal colour as a GPUI colour.
pub fn gpui_color(c: Rgba) -> gpui_kit::Hsla {
    rgba(c.0).into()
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.min_size.height = px(0.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let font = self.view.read(cx).font.clone();
        let (family, fallbacks) = mono_font_stack(&font.face);
        let regular = Font {
            family: family.into(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: Some(FontFallbacks::from_fonts(fallbacks)),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let size = px(font.size);
        let ts = window.text_system();
        let id = ts.resolve_font(&regular);
        let advance = ts
            .advance(id, size, 'W')
            .map(|s| f32::from(s.width))
            .unwrap_or(font.size * 0.6);
        let ascent = f32::from(ts.ascent(id, size));
        let descent = f32::from(ts.descent(id, size)).abs();
        let metrics = CellMetrics::compute(
            advance,
            ascent,
            descent,
            font.size,
            font.line_height,
            window.scale_factor(),
        );

        let fitted = self.view.update(cx, |v, cx| {
            guard::catch("terminal layout", || {
                let layout = v.fit(bounds, metrics, cx);
                let blink_on = v.blink.as_ref().is_none_or(|b| b.is_visible());
                (layout, v.screen.frame(v.focused, blink_on), v.active)
            })
        });
        // A pane whose layout panicked paints nothing and takes no pointer events.
        let (layout, frame, active) = fitted.unwrap_or_else(|| {
            let layout = GridLayout {
                content: bounds,
                origin: bounds.origin,
                metrics,
                cols: 0,
                rows: 0,
            };
            let frame = Frame {
                cols: 0,
                rows: 0,
                background: Rgba::default(),
                rects: Vec::new(),
                glyphs: Vec::new(),
                cursor: None,
            };
            (layout, frame, false)
        });
        let hitboxes = active.then(|| {
            (
                window.insert_hitbox(bounds, HitboxBehavior::Normal),
                window.insert_hitbox(layout.scrollbar_bounds(), HitboxBehavior::Normal),
            )
        });
        Prepaint {
            layout,
            frame,
            fonts: Fonts { regular, size },
            #[cfg(target_os = "macos")]
            atlas_font: FontKey::new(&font.face, &metrics, font.line_height),
            hitboxes,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        p: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = self.view.read(cx);
        let Some((grid_hitbox, scrollbar_hitbox)) = p.hitboxes.clone() else {
            return;
        };
        let l = p.layout;
        let m = l.metrics;
        let dpr = m.dpr;
        let dev = |v: f32| px(v / dpr);
        let cell = |col: usize, row: usize| {
            point(
                l.origin.x + dev(col as f32 * m.cell_width),
                l.origin.y + dev(row as f32 * m.cell_height),
            )
        };
        let mouse_events = view.screen.mouse_events_active();
        let link = view.hovered_link.clone();
        let fg_link = view.screen.foreground();
        let offset = view.screen.display_offset() as i64;
        let composition = view.composition.clone();
        let cursor_cell = view.screen.cursor_in_viewport();
        let now = cx.background_executor().now();
        // xterm's slider fades are CSS transitions without a reduced-motion rule: they always
        // run, and only a parity capture's freeze settles them.
        let opacity = view.scrollbar.opacity_with(|start, dur| {
            if recur_ui::motion::is_frozen(cx) {
                recur_ui::motion::progress(start, dur, cx)
            } else {
                (now.saturating_duration_since(start).as_secs_f32() / dur.as_secs_f32()).min(1.0)
            }
        });
        let slider = view.scrollbar.slider();
        let palette = view.screen.palette().clone();
        let slider_color = if view.scrollbar.is_dragging() {
            palette.scrollbar_slider_active_background
        } else if view.slider_hovered {
            palette.scrollbar_slider_hover_background
        } else {
            palette.scrollbar_slider_background
        };
        let alt_held = window.modifiers().alt;
        let column_select = view.focused && view.should_column_select(alt_held);
        let focus = view.focus.clone();

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // `.xterm` / viewport background: the theme background across the content width.
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.left(), l.origin.y),
                    size(bounds.size.width, px(l.grid_height())),
                ),
                gpui_color(p.frame.background),
            ));
            #[cfg(target_os = "macos")]
            {
                tiles::paint(
                    window,
                    l.origin,
                    &p.frame,
                    &m,
                    &p.atlas_font,
                    p.frame.background,
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                for r in &p.frame.rects {
                    let o = cell(r.col, r.row);
                    window.paint_quad(fill(
                        Bounds::new(
                            o,
                            size(dev(r.len as f32 * m.cell_width), dev(m.cell_height)),
                        ),
                        gpui_color(r.color),
                    ));
                }
                for g in &p.frame.glyphs {
                    paint_glyph(window, g, cell(g.col, g.row), &m, &p.fonts);
                }
            }
            if let Some(link) = &link {
                let (x1, y1) = (
                    link.start.column.0 as i64,
                    link.start.line.0 as i64 + offset,
                );
                let (x2, y2) = (
                    link.end.column.0 as i64 + 1,
                    link.end.line.0 as i64 + offset,
                );
                let cols = l.cols as i64;
                for y in y1..=y2 {
                    if y < 0 || y >= l.rows as i64 {
                        continue;
                    }
                    let (a, b) = (
                        if y == y1 { x1 } else { 0 },
                        if y == y2 { x2 } else { cols },
                    );
                    if b <= a {
                        continue;
                    }
                    let top = (y + 1) as f32 * m.cell_height - dpr - 1.0;
                    let o = point(
                        l.origin.x + dev(a as f32 * m.cell_width),
                        l.origin.y + dev(y as f32 * m.cell_height),
                    );
                    let o = point(o.x, l.origin.y + dev(top));
                    window.paint_quad(fill(
                        Bounds::new(o, size(dev((b - a) as f32 * m.cell_width), dev(dpr))),
                        gpui_color(fg_link),
                    ));
                }
            }
            #[cfg(not(target_os = "macos"))]
            if let Some(c) = p.frame.cursor {
                paint_cursor(
                    window,
                    cell(c.col, c.row),
                    c.width,
                    c.shape,
                    gpui_color(c.color),
                    &m,
                );
            }
            if let (Some(comp), Some((cx_, cy))) = (&composition, cursor_cell) {
                paint_composition(window, cx, cell(cx_, cy), &comp.text, &p.fonts, &l, cx_);
            }
            if let Some(s) = slider
                && opacity > 0.0
            {
                let sb = l.scrollbar_bounds();
                let color = slider_color.multiply_opacity(opacity as f64);
                window.paint_quad(fill(
                    Bounds::new(
                        point(sb.left(), sb.top() + px(s.position)),
                        size(sb.size.width, px(s.size)),
                    ),
                    gpui_color(color),
                ));
            }
        });

        let style = if link.is_some() {
            CursorStyle::PointingHand
        } else if mouse_events {
            CursorStyle::Arrow
        } else if column_select {
            CursorStyle::Crosshair
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(style, &grid_hitbox);
        window.set_cursor_style(CursorStyle::Arrow, &scrollbar_hitbox);
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        let (v, hitbox) = (self.view.clone(), grid_hitbox.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            v.update(cx, |v, cx| {
                guard::catch("terminal mouse_down", || {
                    v.window_mouse_down(cx.background_executor().now())
                });
            });
            if hitbox.is_hovered(window) {
                v.update(cx, |v, cx| {
                    guard::catch("terminal mouse_down", || {
                        v.mouse_down(e.position, e.button, e.click_count, e.modifiers, window, cx)
                    });
                });
            }
        });
        let (v, hitbox) = (self.view.clone(), grid_hitbox.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = hitbox.is_hovered(window);
            v.update(cx, |v, cx| {
                guard::catch("terminal mouse_move", || {
                    v.mouse_move(e.position, e.pressed_button, e.modifiers, hovered, cx)
                });
            });
        });
        let v = self.view.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            v.update(cx, |v, cx| {
                guard::catch("terminal mouse_up", || {
                    v.mouse_up(e.position, e.button, e.modifiers, cx)
                });
            });
        });
        let v = self.view.clone();
        window.on_mouse_event(move |_: &MouseExitEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |v, cx| {
                    guard::catch("terminal mouse_exited", || v.mouse_exited(cx));
                });
            }
        });
        let (v, hitbox) = (self.view.clone(), grid_hitbox.clone());
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                return;
            }
            v.update(cx, |v, cx| {
                guard::catch("terminal scroll_wheel", || v.scroll_wheel(e, cx));
            });
        });
    }
}

#[cfg(not(target_os = "macos"))]
fn paint_glyph(
    window: &mut Window,
    g: &Glyph,
    origin: Point<Pixels>,
    m: &CellMetrics,
    fonts: &Fonts,
) {
    let dpr = m.dpr;
    let dev = |v: f32| px(v / dpr);
    let color = gpui_color(g.fg);
    if !custom_glyphs::paint(window, g.codepoint, origin, g.width, m, color) && g.text != " " {
        let font = fonts.variant(g.bold, g.italic);
        let text: SharedString = g.text.clone().into();
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(text, fonts.size, &[run], None);
        let baseline = origin.y + dev(m.baseline());
        for r in shaped.runs.iter() {
            for glyph in &r.glyphs {
                let o = point(origin.x + glyph.position.x, baseline);
                let _ = if glyph.is_emoji {
                    window.paint_emoji(o, r.font_id, glyph.id, fonts.size)
                } else {
                    window.paint_glyph(o, r.font_id, glyph.id, fonts.size, color)
                };
            }
        }
    }
    let box_top = origin.y + dev(m.char_top);
    let width = g.width as f32 * m.cell_width;
    if let Some(u) = g.underline {
        paint_underline(
            window,
            origin.x,
            box_top,
            width,
            u.style,
            gpui_color(u.color),
            m,
            origin,
        );
    }
    if g.overline {
        // TextureAtlas strokes it along the top of the character box, half a pixel down when
        // the width is odd, in the glyph colour.
        let lw = m.underline_width();
        let y_off = if lw as i32 % 2 == 1 { 0.5 } else { 0.0 };
        window.paint_quad(fill(
            Bounds::new(
                point(origin.x, box_top + dev(y_off - lw / 2.0)),
                size(dev(g.width as f32 * m.char_width), dev(lw)),
            ),
            color,
        ));
    }
    if g.strikethrough {
        let lw = m.strike_width();
        // The atlas checks the previous (odd) context line width, so the stroke is centred on a
        // half pixel.
        let center = (m.char_height / 2.0).floor() - 0.5;
        window.paint_quad(fill(
            Bounds::new(
                point(origin.x, box_top + dev(center - lw / 2.0)),
                size(dev(g.width as f32 * m.char_width), dev(lw)),
            ),
            color,
        ));
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn paint_underline(
    window: &mut Window,
    x: Pixels,
    box_top: Pixels,
    width: f32,
    style: UnderlineStyle,
    color: gpui_kit::Hsla,
    m: &CellMetrics,
    origin: Point<Pixels>,
) {
    let dpr = m.dpr;
    let dev = |v: f32| px(v / dpr);
    let lw = m.underline_width();
    let y_off = if lw as i32 % 2 == 1 { 0.5 } else { 0.0 };
    let y_top = m.char_height.ceil() - y_off;
    let line = |window: &mut Window, y: f32, x0: f32, x1: f32| {
        if x1 > x0 {
            window.paint_quad(fill(
                Bounds::new(
                    point(x + dev(x0), box_top + dev(y - lw / 2.0)),
                    size(dev(x1 - x0), dev(lw)),
                ),
                color,
            ));
        }
    };
    match style {
        UnderlineStyle::Single => line(window, y_top, 0.0, width),
        UnderlineStyle::Double => {
            line(window, y_top, 0.0, width);
            line(window, y_top + lw * 2.0, 0.0, width);
        }
        UnderlineStyle::Dashed => {
            let cw = m.cell_width;
            let mut cx = 0.0;
            while cx < width {
                let dash = (0.6 * cw).floor();
                let gap = (0.3 * cw).floor();
                line(window, y_top, cx, cx + dash);
                line(window, y_top, cx + dash + gap, cx + cw);
                cx += cw;
            }
        }
        UnderlineStyle::Dotted => {
            // A dash pattern of `lw` on / `lw` off, continuous across cells.
            let step = lw.round().max(1.0);
            let abs_x = f32::from(origin.x) * dpr;
            let mut d = 0.0;
            while d < width {
                if (((abs_x + d) / step).floor() as i64) % 2 == 0 {
                    line(window, y_top, d, (d + step).min(width));
                }
                d += step;
            }
        }
        UnderlineStyle::Curly => {
            let y0 = m.char_height + 1.0;
            let y1 = y0 + 3.0 * dpr;
            let cw = m.cell_width;
            let (sx, sy) = (cw / 6.0, (y1 - y0) / 3.0);
            const POLYS: [[f32; 8]; 3] = [
                [0.0, 2.0, 1.0, 3.0, 2.4, 3.0, 0.0, 0.6],
                [5.5, 0.0, 2.5, 3.0, 1.1, 3.0, 4.1, 0.0],
                [4.0, 0.0, 6.0, 2.0, 6.0, 0.6, 5.4, 0.0],
            ];
            let mut cx = 0.0;
            while cx < width {
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: Bounds::new(
                            point(x + dev(cx), box_top + dev(y0)),
                            size(dev(cw), dev(y1 - y0)),
                        ),
                    }),
                    |window| {
                        for poly in POLYS {
                            let mut b = gpui_kit::PathBuilder::fill();
                            for i in (0..8).step_by(2) {
                                let p = point(
                                    x + dev(cx + poly[i] * sx),
                                    box_top + dev(y1 - poly[i + 1] * sy),
                                );
                                if i == 0 {
                                    b.move_to(p);
                                } else {
                                    b.line_to(p);
                                }
                            }
                            b.close();
                            if let Ok(path) = b.build() {
                                window.paint_path(path, color);
                            }
                        }
                    },
                );
                cx += cw;
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn paint_cursor(
    window: &mut Window,
    origin: Point<Pixels>,
    width: usize,
    shape: CursorShape,
    color: gpui_kit::Hsla,
    m: &CellMetrics,
) {
    let dpr = m.dpr;
    let dev = |v: f32| px(v / dpr);
    let w = width as f32 * m.cell_width;
    let h = m.cell_height;
    let mut rect = |x: f32, y: f32, rw: f32, rh: f32| {
        window.paint_quad(fill(
            Bounds::new(
                point(origin.x + dev(x), origin.y + dev(y)),
                size(dev(rw), dev(rh)),
            ),
            color,
        ));
    };
    match shape {
        // cursorWidth 1: one CSS pixel.
        CursorShape::Bar => rect(0.0, 0.0, dpr, h),
        CursorShape::Underline => rect(0.0, h - dpr, w, dpr),
        CursorShape::Outline => {
            rect(0.0, 0.0, dpr, h);
            rect(0.0, h - dpr, w, dpr);
            rect(0.0, 0.0, w, dpr);
            rect(w - dpr, 0.0, dpr, h);
        }
    }
}

/// The IME composition view: white on black at the cursor, one cell tall, clipped to the grid's
/// right edge with the end of the text visible (`direction: rtl`).
fn paint_composition(
    window: &mut Window,
    cx: &mut App,
    origin: Point<Pixels>,
    text: &str,
    fonts: &Fonts,
    l: &GridLayout,
    cursor_x: usize,
) {
    let text: SharedString = text.to_string().into();
    let run = TextRun {
        len: text.len(),
        font: fonts.regular.clone(),
        color: gpui_kit::white(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(text, fonts.size, &[run], None);
    let max = px((l.cols - cursor_x) as f32 * l.cell_w());
    let width = shaped.width.min(max);
    let h = px(l.cell_h());
    let bounds = Bounds::new(origin, size(width, h));
    window.paint_quad(fill(bounds, gpui_kit::black()));
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let x = origin.x + width - shaped.width;
        let _ = shaped.paint(
            point(x, origin.y),
            h,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
    });
}
