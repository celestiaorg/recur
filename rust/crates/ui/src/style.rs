//! Generic styles from `_base.scss`, `_forms.scss`, `_overlays.scss` (and the `.tbtn` / `.rbtn`
//! buttons they build on), as GPUI style builders. Each function documents the class it
//! ports; hover, active and keyboard-focus states are attached the same way the CSS cascades
//! them. View-specific rules stay with their views.

use crate::color::{Color, rgba};
use crate::fonts::{Family, MONO, UI, weight};
use crate::shadow::{CssShadow, css_box_shadow};
use crate::text::{InlineBox, LineHeight, Span, Text, TextStyled, span, text};
use crate::theme::{Theme, Tokens};
use gpui_kit::base::input::{Input, InputEditorStyle, InputState};
use gpui_kit::{
    App, BoxShadow, Div, ElementId, Entity, Fill, FocusHandle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, Stateful, Styled, Window, div, point,
    prelude::FluentBuilder, px, relative,
};
use std::sync::atomic::{AtomicBool, Ordering};

impl From<Color> for Fill {
    fn from(c: Color) -> Fill {
        Hsla::from(c).into()
    }
}

fn tokens(cx: &App) -> Tokens {
    Theme::of(cx).tokens
}

/// CSS `opacity` fades an element and its content as one layer. GPUI applies opacity to each
/// primitive, which lets a label or border show through its own fill, so faded controls
/// flatten their colors instead: each color as composited inside the element (over `fill`),
/// then mixed with the `backdrop` the control sits on. Exact for opaque backdrops, glyph
/// anti-aliasing included.
pub fn faded(color: Color, fill: Color, opacity: f32, backdrop: Color) -> Color {
    color.over(fill.over(backdrop)).mix(opacity, backdrop)
}

/// A border the same color as its opaque fill paints nothing visible; drop it so GPUI's
/// per-primitive `opacity` (CSS fades the element as one layer) does not double it up.
fn border_over(border: Color, bg: Color) -> Hsla {
    if border == bg {
        Hsla::transparent_black()
    } else {
        border.into()
    }
}

/// A CSS `linear-gradient(<angle>, from, to)` background for its relatively positioned parent with a `border` of
/// `border_color` and `radius`. See [`inset_background`] for its extent. Painted as a raster of
/// its device pixels: WebKit's gradients are smooth, GPUI's shader dithers them by ±2 levels.
pub fn css_gradient(
    angle: f32,
    from: (Color, f32),
    to: (Color, f32),
    radius: f32,
    border: f32,
    border_color: Color,
) -> impl IntoElement {
    let opaque = border_color.a >= 1.;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let gradient_box = bounds.dilate(px(border));
            let (bounds, radius) = background_box(bounds, radius, border, opaque, window);
            let g = Gradient {
                angle,
                from,
                to,
                line: gradient_box,
            };
            if let Some(image) = gradient_image(&g, bounds, window.scale_factor()) {
                let _ = window.paint_image(
                    bounds,
                    bounds,
                    gpui_kit::Corners::all(px(radius)),
                    image,
                    0,
                    false,
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A CSS `linear-gradient(<angle>, from, to)` filling its parent with square corners, in place
/// of `.bg(linear_gradient(..))` (see [`css_gradient`]). Put it first.
pub fn gradient_fill(angle: f32, from: (Color, f32), to: (Color, f32)) -> impl IntoElement {
    css_gradient(angle, from, to, 0., 0., crate::color::TRANSPARENT)
}

/// Paint a CSS `linear-gradient(<angle>, from, to)` over `bounds` (its gradient box) with
/// `radius` corners, undithered (see [`css_gradient`]).
pub fn paint_css_gradient(
    window: &mut Window,
    bounds: gpui_kit::Bounds<Pixels>,
    radius: f32,
    angle: f32,
    from: (Color, f32),
    to: (Color, f32),
) {
    let g = Gradient {
        angle,
        from,
        to,
        line: bounds,
    };
    if let Some(image) = gradient_image(&g, bounds, window.scale_factor()) {
        let _ = window.paint_image(
            bounds,
            bounds,
            gpui_kit::Corners::all(px(radius)),
            image,
            0,
            false,
        );
    }
}

/// A two-stop CSS `linear-gradient` over the box `line` (its gradient box).
#[derive(Clone, Copy, PartialEq)]
struct Gradient {
    angle: f32,
    from: (Color, f32),
    to: (Color, f32),
    line: gpui_kit::Bounds<Pixels>,
}

impl Gradient {
    /// The 8-bit color at `p` (CSS px), interpolated in premultiplied sRGB as CSS does.
    fn at(&self, p: (f32, f32)) -> [u8; 4] {
        let a = self.angle.to_radians();
        let (dx, dy) = (a.sin(), -a.cos());
        let (w, h) = (
            f32::from(self.line.size.width),
            f32::from(self.line.size.height),
        );
        let c = self.line.center();
        let len = (w * dx).abs() + (h * dy).abs();
        let t = ((p.0 - f32::from(c.x)) * dx + (p.1 - f32::from(c.y)) * dy) / len.max(1e-6) + 0.5;
        let (c0, s0) = self.from;
        let (c1, s1) = self.to;
        let u = if s1 > s0 {
            ((t - s0) / (s1 - s0)).clamp(0., 1.)
        } else if t < s0 {
            0.
        } else {
            1.
        };
        let alpha = c0.a + (c1.a - c0.a) * u;
        let ch = |x0: f32, x1: f32| {
            let pm = x0 * c0.a + (x1 * c1.a - x0 * c0.a) * u;
            if alpha > 0. { pm / alpha } else { 0. }
        };
        Color {
            r: ch(c0.r, c1.r),
            g: ch(c0.g, c1.g),
            b: ch(c0.b, c1.b),
            a: alpha,
        }
        .to_rgba8()
    }
}

type GradientKey = (u32, u32, [u32; 12]);

thread_local! {
    static GRADIENTS: std::cell::RefCell<
        std::collections::HashMap<GradientKey, std::sync::Arc<gpui_kit::RenderImage>>,
    > = Default::default();
}

/// The gradient's pixels over `bounds` (device-aligned), cached by size and gradient.
fn gradient_image(
    g: &Gradient,
    bounds: gpui_kit::Bounds<Pixels>,
    scale: f32,
) -> Option<std::sync::Arc<gpui_kit::RenderImage>> {
    let (w, h) = (
        (f32::from(bounds.size.width) * scale).round() as u32,
        (f32::from(bounds.size.height) * scale).round() as u32,
    );
    if w == 0 || h == 0 {
        return None;
    }
    // Relative to the painted box, so a moved box reuses its raster.
    let (ox, oy) = (
        f32::from(g.line.origin.x - bounds.origin.x),
        f32::from(g.line.origin.y - bounds.origin.y),
    );
    let bits = |c: Color| u32::from_le_bytes(c.to_rgba8());
    let key = (
        w,
        h,
        [
            g.angle.to_bits(),
            bits(g.from.0),
            g.from.1.to_bits(),
            bits(g.to.0),
            g.to.1.to_bits(),
            ox.to_bits(),
            oy.to_bits(),
            f32::from(g.line.size.width).to_bits(),
            f32::from(g.line.size.height).to_bits(),
            scale.to_bits(),
            0,
            0,
        ],
    );
    if let Some(hit) = GRADIENTS.with(|m| m.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let local = Gradient {
        line: gpui_kit::Bounds {
            origin: point(px(ox), px(oy)),
            size: g.line.size,
        },
        ..*g
    };
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, gr, b, a] = local.at(((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale));
            bgra.extend_from_slice(&[b, gr, r, a]);
        }
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, bgra)?);
    let image = std::sync::Arc::new(gpui_kit::RenderImage::new(smallvec::SmallVec::from_elem(
        frame, 1,
    )));
    GRADIENTS.with(|m| {
        let mut m = m.borrow_mut();
        // Sizes change while windows resize; keep the cache from growing without bound.
        if m.len() > 256 {
            m.clear();
        }
        m.insert(key, image.clone());
    });
    Some(image)
}

/// The background of a relatively positioned parent with a `border` of `border_color` and
/// `radius`, in place of `.bg()`. GPUI paints a div's background and border as two anti-aliased
/// quads, so the background bleeds into the border's outer edge; under an opaque border WebKit
/// shrinks the background one device pixel inside the border box instead
/// (`BackgroundBleedShrinkBackground`). Put it first.
pub fn inset_background(
    color: Color,
    radius: f32,
    border: f32,
    border_color: Color,
) -> impl IntoElement {
    let opaque = border_color.a >= 1.;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (bounds, radius) = background_box(bounds, radius, border, opaque, window);
            window.paint_quad(gpui_kit::fill(bounds, Hsla::from(color)).corner_radii(px(radius)));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The border box from a canvas filling the padding box, shrunk by a device pixel under an
/// opaque border.
fn background_box(
    padding_box: gpui_kit::Bounds<Pixels>,
    radius: f32,
    border: f32,
    opaque: bool,
    window: &Window,
) -> (gpui_kit::Bounds<Pixels>, f32) {
    let shrink = if border > 0. && opaque {
        1. / window.scale_factor()
    } else {
        0.
    };
    (
        padding_box.dilate(px(border - shrink)),
        (radius - shrink).max(0.),
    )
}

/// A CSS `box-shadow` layer. CSS blur is twice the Gaussian sigma GPUI takes.
pub fn css_shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color) -> BoxShadow {
    BoxShadow {
        color: color.into(),
        offset: point(px(x), px(y)),
        blur_radius: px(blur / 2.),
        spread_radius: px(spread),
        inset: false,
    }
}

/// `body`: `--bg` background, `--text` color, Inter 13px, line-height 1.45.
pub fn body(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .bg(t.bg)
        .text_color(t.text)
        .font_family(UI)
        .text_size(px(13.))
        .line_height(relative(1.45))
}

/// How far [`window_root`] shifts the tree so half-pixel ties round down and right, as WebKit's
/// do. A cached view lays out as its own root and applies it again inside.
pub const TIE_NUDGE: f32 = 1. / 4096.;

/// `anchored()` at a fractional window position, as WebKit places a box. GPUI rounds an anchored
/// offset to whole CSS pixels; this anchors at the whole pixel and moves the child the exact rest
/// of the way with `aspect_ratio` spacers (authored lengths would round), so the child and its
/// descendants keep their fractional positions and snap as WebKit snaps them. Set `.anchor(..)`
/// on the result as usual.
pub fn anchored_at(x: f32, y: f32, child: impl IntoElement) -> gpui_kit::Anchored {
    let (x, y) = (x + TIE_NUDGE, y + TIE_NUDGE);
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    gpui_kit::anchored().position(point(px(ix), px(iy))).child(
        div()
            .flex()
            .flex_col()
            .child(div().flex_none().w(px(1.)).aspect_ratio(1. / fy))
            .child(
                div()
                    .flex()
                    .child(div().flex_none().h(px(1.)).aspect_ratio(fx))
                    .child(child),
            ),
    )
}

/// `flex: <grow> 1 0` on a padded or bordered item. CSS takes a zero flex basis as the item's
/// border box without flooring it at its padding and border, so items with different padding
/// still share the free space equally; Taffy floors the base size at padding + border. An
/// unpadded wrapper carries the share and the item fills it.
pub fn flex_share(grow: f32, item: impl IntoElement) -> Div {
    div()
        .flex_grow(grow)
        .flex_shrink(1.)
        .flex_basis(gpui_kit::relative(0.))
        .flex()
        .child(item)
}

/// A menu or popover attached to its trigger, placed the way CSS places one from the trigger's
/// fractional rect: absolutely positioned in the trigger's relatively positioned box, `gap`
/// below its bottom edge (or above its top edge when `below` is false) and `dx` right of its
/// left edge. Staying in the layout tree keeps the trigger's fractional position, where
/// [`anchored_at`] can only start from its snapped bounds (and, anchored at a bottom edge,
/// subtracts the overlay's snapped height). Wrap it in `deferred` to paint it above later
/// siblings.
pub fn attached(below: bool, gap: f32, dx: f32, child: impl IntoElement) -> Div {
    let d = div().absolute().left(px(dx));
    let d = if below {
        d.top(gpui_kit::relative(1.)).mt(px(gap))
    } else {
        d.bottom(gpui_kit::relative(1.)).mb(px(gap))
    };
    d.child(child)
}

/// `deferred` that keeps the clip it was laid out under: CSS `z-index` lifts a popover above
/// later siblings but its scroll container's `overflow` still clips it, where `deferred` paints
/// unclipped.
pub fn deferred_clipped(child: impl IntoElement, priority: usize) -> ClippedDeferred {
    ClippedDeferred {
        child: Some(child.into_any_element()),
        priority,
    }
}

pub struct ClippedDeferred {
    child: Option<gpui_kit::AnyElement>,
    priority: usize,
}

impl IntoElement for ClippedDeferred {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui_kit::Element for ClippedDeferred {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui_kit::LayoutId, ()) {
        let id = match self.child.as_mut() {
            Some(c) => c.request_layout(window, cx),
            None => window.request_layout(gpui_kit::Style::default(), [], cx),
        };
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        if let Some(child) = self.child.take() {
            let mask = window.content_mask();
            window.defer_draw(child, window.element_offset(), self.priority, Some(mask));
        }
    }

    fn paint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

/// The root every window renders its content in. WebKit snaps a box edge that lands exactly on
/// a half device pixel down and to the right; GPUI rounds halves toward zero. Shifting the whole
/// tree by 1/4096 px (through `aspect_ratio` spacers, since authored lengths round to whole
/// device pixels) makes those ties round as WebKit does and moves nothing else.
pub fn window_root(content: impl IntoElement) -> Div {
    const NUDGE: f32 = 1. / TIE_NUDGE;
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().flex_none().w(px(1.)).aspect_ratio(NUDGE))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_none().h(px(1.)).aspect_ratio(1. / NUDGE))
                .child(div().flex_1().min_w_0().size_full().child(content)),
        )
}

/// `entity` as a cached view laid out at `style` (see `rust/docs/ARCHITECTURE.md`, "Frame
/// cost"). It reads the entity first: GPUI only invalidates a view the window saw accessed in a
/// draw, and a cached view's own render does not count when the entity was already touched
/// earlier in that frame (as on the frame its parent created it). Without the read, a view
/// reused from then on drops its own notifies until something refreshes the whole window.
pub fn cached_view<V: gpui_kit::Render>(
    entity: &gpui_kit::Entity<V>,
    style: gpui_kit::StyleRefinement,
    cx: &App,
) -> gpui_kit::AnyElement {
    let _ = entity.read(cx);
    entity.clone().cached(style).into_any_element()
}

/// `entity` as an isolated view laid out at `style`: a notify of it, or of a view inside it,
/// renders only it, in place, while the frame replays its ancestors (Recur's gpui-pre patch,
/// `rust/vendor/README.md`). Its size comes from `style`, never its content: a view that needs
/// another size notifies its host. Reads the entity first, as [`cached_view`] does.
pub fn isolated_view<V: gpui_kit::Render>(
    entity: &gpui_kit::Entity<V>,
    style: gpui_kit::StyleRefinement,
    cx: &App,
) -> gpui_kit::AnyElement {
    let _ = entity.read(cx);
    entity.clone().isolated(style).into_any_element()
}

/// The root of a cached view's content: [`window_root`]'s tie nudge (a cached view lays out as
/// its own root from a snapped origin, without the window's) around a column the content fills.
pub fn cached_root(content: impl IntoElement) -> Div {
    const NUDGE: f32 = 1. / TIE_NUDGE;
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().flex_none().w(px(1.)).aspect_ratio(NUDGE))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_none().h(px(1.)).aspect_ratio(1. / NUDGE))
                .child(div().flex_1().min_w_0().flex().flex_col().child(content)),
        )
}

/// Button label text: `<button>` resets `font` (13.333px, line-height normal) and the class
/// then sets size and weight.
pub fn button_label(label: impl Into<gpui_kit::SharedString>, size: f32, weight: f32) -> Text {
    text(label)
        .size(size)
        .weight(weight)
        .line_height(LineHeight::Normal)
        .nowrap()
}

/// Whether `handle` should paint its `:focus-visible` treatment: focused, and reached by
/// keyboard (GPUI's input-modality tracking, the same rule browsers use).
pub fn is_focus_visible(handle: &FocusHandle, window: &Window) -> bool {
    handle.is_focused(window) && window.last_input_was_keyboard()
}

/// The global `:focus-visible` ring: a 2px `--amber` 70% outline 1px outside a box with corner
/// `radius` and a `border`-wide border (`:focus-visible` also sets `border-radius: 4px`, so pass
/// 4 unless a more specific rule keeps the box's own). A child of the relatively positioned box.
pub fn focus_ring(radius: f32, border: f32, cx: &App) -> Div {
    outline(2., 1., radius, border, tokens(cx).amber.fade(0.7))
}

/// A CSS outline of `width` at `offset` around a box with corner `radius` and a `border`-wide
/// border, as an absolutely positioned overlay child of that box.
pub fn outline(width: f32, offset: f32, radius: f32, border: f32, color: Color) -> Div {
    let inset = -(offset + width + border);
    div()
        .absolute()
        .top(px(inset))
        .left(px(inset))
        .right(px(inset))
        .bottom(px(inset))
        .border(px(width))
        .border_color(Hsla::from(color))
        .rounded(px(radius + offset + width))
}

/// `.tbtn` variants (`_main.scss`, `_observatory.scss`, `_overlays.scss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tbtn {
    #[default]
    Default,
    /// `.tbtn.amber`
    Amber,
    /// `.tbtn.solid`
    Solid,
    /// `.tbtn.on` (the `_observatory.scss` rule, which cascades last)
    On,
    /// `.tbtn.danger`
    Danger,
    /// `.tbtn.danger.armed`
    Armed,
}

/// Metrics of a `.tbtn`: the base class, or `.tb-right .tbtn` in the titlebar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TbtnSize {
    pub height: f32,
    pub pad_x: f32,
    pub font_size: f32,
}

impl TbtnSize {
    pub const BASE: TbtnSize = TbtnSize {
        height: 28.,
        pad_x: 11.,
        font_size: 12.,
    };
    /// `.tb-right .tbtn`.
    pub const TITLEBAR: TbtnSize = TbtnSize {
        height: 26.,
        pad_x: 9.,
        font_size: 11.5,
    };
}

/// `.tbtn`: inline-flex, gap 6, radius 6, 1px border, weight 500. Children: icons at 13px and
/// a [`button_label`]. `disabled_over` disables it; titlebar buttons also fade over that
/// backdrop.
pub fn tbtn(
    id: impl Into<ElementId>,
    kind: Tbtn,
    size: TbtnSize,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut bg, mut border, mut fg) = match kind {
        Tbtn::Default => (t.pane, t.line, t.text),
        Tbtn::Amber => (t.amber, t.amber, t.on_amber),
        Tbtn::Solid => (t.text, t.text, t.bg),
        Tbtn::On => (t.pane, t.amber, t.amber),
        Tbtn::Danger => (t.pane, rgba(235, 122, 92, 0.32), t.coral),
        Tbtn::Armed => (t.coral, t.coral, crate::color::hex(0x0a0c10)),
    };
    // `.tb-right .tbtn:disabled { opacity: .45; cursor: not-allowed }`; elsewhere a disabled
    // `.tbtn` looks enabled.
    if let Some(back) = disabled_over
        && size == TbtnSize::TITLEBAR
    {
        (bg, border, fg) = (
            faded(bg, bg, 0.45, back),
            faded(border, bg, 0.45, back),
            faded(fg, bg, 0.45, back),
        );
    }
    let disabled = disabled_over.is_some();
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(size.height))
        .px(px(size.pad_x))
        .rounded(px(6.))
        .border(px(1.))
        .border_color(border_over(border, bg))
        .bg(bg)
        .text_color(fg)
        .text_size(px(size.font_size))
        .font_weight(gpui_kit::FontWeight(weight::MEDIUM));
    if disabled {
        b = b.cursor_not_allowed();
    } else {
        b = b.cursor_pointer();
        match kind {
            // `.tbtn:hover` only changes the border; danger/armed/amber keep theirs (later or
            // more specific rules win).
            Tbtn::Default => b = b.hover(|s| s.border_color(Hsla::from(t.line_bright))),
            // `.tbtn.solid:hover { filter: brightness(.92) }` dims fill, border and label.
            Tbtn::Solid => {
                let (dim, ink) = (t.text.brightness(0.92), t.bg.brightness(0.92));
                b = b.hover(move |s| s.bg(dim).border_color(Hsla::from(dim)).text_color(ink))
            }
            _ => {}
        }
    }
    b
}

/// `border-right: 1px solid rgba(0, 0, 0, .15)` on a split button's main half (`.merge-split`,
/// `.update-split`). GPUI borders take one color, so the darker right edge is a strip over the
/// button's own border; a child of the relatively positioned button.
pub fn split_divider() -> Div {
    div()
        .absolute()
        .top(px(-1.))
        .bottom(px(-1.))
        .right(px(-1.))
        .w(px(1.))
        .bg(rgba(0, 0, 0, 0.15))
}

/// `.rbtn` variants (`_prs.scss`, `_observatory.scss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rbtn {
    #[default]
    Default,
    /// `.rbtn.merge`
    Merge,
    /// `.rbtn.primary`
    Primary,
    /// `.rbtn.armed`
    Armed,
}

/// `.rbtn`: height 30, padding 0 13, radius 7, 12.5px weight 550, gap 7.
pub fn rbtn(
    id: impl Into<ElementId>,
    kind: Rbtn,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (mut bg, mut border, mut fg) = match kind {
        Rbtn::Default => (t.pane, t.line, t.text),
        Rbtn::Merge => (t.emerald, t.emerald, crate::color::hex(0x0a0c10)),
        Rbtn::Primary => (t.text, t.text, t.bg),
        Rbtn::Armed => (t.coral, t.coral, crate::color::hex(0x0a0c10)),
    };
    // `.rbtn.primary:disabled { opacity: .35 }`; other rbtns fade like titlebar buttons.
    if let Some(back) = disabled_over {
        let o = if kind == Rbtn::Primary { 0.35 } else { 0.45 };
        (bg, border, fg) = (
            faded(bg, bg, o, back),
            faded(border, bg, o, back),
            faded(fg, bg, o, back),
        );
    }
    let disabled = disabled_over.is_some();
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .h(px(30.))
        .px(px(13.))
        .rounded(px(7.))
        .border(px(1.))
        .border_color(border_over(border, bg))
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.5))
        .font_weight(gpui_kit::FontWeight(weight::MEDIUM_PLUS));
    if !disabled {
        b = b.cursor_pointer();
        if kind == Rbtn::Default {
            b = b.hover(|s| s.border_color(Hsla::from(t.line_bright)));
        }
    }
    b
}

/// `.nt-go`: the solid primary action (Create, Approve). Text-colored fill, 32px, radius 8,
/// pushed to the end of its row (`margin-left: auto`).
pub fn primary_button(
    id: impl Into<ElementId>,
    disabled_over: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    // `.nt-go:disabled { opacity: .35 }`.
    let (bg, fg) = match disabled_over {
        Some(back) => (
            faded(t.text, t.text, 0.35, back),
            faded(t.bg, t.text, 0.35, back),
        ),
        None => (t.text, t.bg),
    };
    let mut b = div()
        .id(id)
        .ml_auto()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .font_weight(gpui_kit::FontWeight(weight::SEMIBOLD));
    if disabled_over.is_none() {
        // `filter: brightness(1.05)` brightens the fill and the label.
        let (lit, ink) = (t.text.brightness(1.05), t.bg.brightness(1.05));
        b = b.cursor_pointer().hover(move |s| s.bg(lit).text_color(ink));
    }
    b
}

/// `.nt-link`: a borderless 30px text action, amber when `on`.
pub fn link_button(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(8.))
        .rounded(px(6.))
        .text_color(if on { t.amber } else { t.faint })
        .text_size(px(12.))
        .cursor_pointer()
        .hover(move |s| {
            s.bg(t.pane_2)
                .text_color(Hsla::from(if on { t.amber } else { t.text }))
        })
}

/// `.lnk`: mono 11px amber text link (underline on hover is the label's job).
pub fn lnk(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Text {
    text(label)
        .mono()
        .size(11.)
        .color(tokens(cx).amber)
        .nowrap()
}

/// `.tb-icon-btn`: 26px-high square-ish titlebar icon button, faint until hover or `on`.
pub fn titlebar_icon_button(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(1.))
        .h(px(26.))
        .px(px(6.))
        .rounded(px(6.))
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)));
    if on {
        b = b.bg(t.pane_2).text_color(t.text);
    }
    b
}

/// `.tb-rail`: the 28px sidebar toggle.
pub fn rail_toggle(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .ml(px(2.))
        .rounded(px(6.))
        .text_color(if on { t.text } else { t.faint })
        .cursor_pointer()
        .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)))
}

/// `.tog` (36×20 switch): knob 14px at left 2, amber track and dark knob at left 18 when on.
pub fn toggle(id: impl Into<ElementId>, on: bool, disabled: bool, cx: &App) -> Stateful<Div> {
    let t = tokens(cx);
    let (track, border, knob, left) = if on {
        (t.amber, t.amber, crate::color::hex(0x0a0c10), 18.)
    } else {
        (t.pane_2, t.line, t.muted, 2.)
    };
    let mut d = div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(36.))
        .h(px(20.))
        .rounded(px(12.))
        .bg(track)
        .border(px(1.))
        .border_color(border_over(border, track))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(left))
                .size(px(14.))
                .rounded_full()
                .bg(knob),
        );
    if !disabled {
        d = d.cursor_pointer();
    }
    d
}

/// `.panel`.
pub fn panel(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .bg(t.pane)
        .border(px(1.))
        .border_color(Hsla::from(t.line))
        .rounded(px(8.))
}

static BACKDROP_BLUR: AtomicBool = AtomicBool::new(true);

/// Turns [`backdrop_blur`]'s blur on or off. The parity renderer turns it off: Playwright's
/// WebKit captures leave backdrop filters out, so the references show scrims unblurred.
pub fn set_backdrop_blur(on: bool) {
    BACKDROP_BLUR.store(on, Ordering::Relaxed);
}

/// `backdrop-filter: blur(radius)` under a `background` filling the parent: an absolute child
/// that blurs what is painted behind the parent, then fills it. Off macOS only the fill shows.
pub fn backdrop_blur(radius: f32, background: Color) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if BACKDROP_BLUR.load(Ordering::Relaxed) {
                window.paint_backdrop_blur(bounds, Default::default(), px(radius));
            }
            window.paint_quad(gpui_kit::fill(bounds, Hsla::from(background)));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The overlay scrim, `rgba(5, 7, 10, .55)` with a 2px backdrop blur, centering its card at
/// the top.
fn scrim() -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(backdrop_blur(2., rgba(5, 7, 10, 0.55)))
        .flex()
        .justify_center()
        .items_start()
}

/// `.nt-bg`: the modal scrim, `padding: 22vh 16px 24px`.
pub fn modal_scrim(window: &Window, panel: impl IntoElement) -> Div {
    vh_scrim(22., None, window, panel).px(px(16.)).pb(px(24.))
}

/// `.palette-bg` around a [`palette_panel`] (`max-height: 64vh`).
pub fn palette_scrim(window: &Window, panel: impl IntoElement) -> Div {
    vh_scrim(11., Some(64.), window, panel)
}

/// `.palette-bg` around a panel at most `max_vh` tall (`.cmd-palette` 56vh, `.shortcuts-panel`
/// 72vh).
pub fn palette_scrim_capped(max_vh: f32, window: &Window, panel: impl IntoElement) -> Div {
    vh_scrim(11., Some(max_vh), window, panel)
}

/// A viewport scrim whose panel hangs `top` vh below its top (`padding-top: 11vh` on a
/// centering flex row) and is at most `max` vh tall, both at the exact LayoutUnits WebKit
/// resolves them to: authored lengths would round, a percentage of the scrim reaches layout
/// as is. The panel never shrinks below its content otherwise, as in a flex row.
fn vh_scrim(top: f32, max: Option<f32>, window: &Window, panel: impl IntoElement) -> Div {
    let vh = f32::from(window.viewport_size().height);
    let top = crate::exact::lu(top * vh / 100.);
    let mut slot = div().flex_none().flex().flex_col();
    if let Some(m) = max {
        slot = slot.max_h(gpui_kit::relative(crate::exact::lu(m * vh / 100.) / vh));
    }
    scrim()
        .flex_col()
        .justify_start()
        .items_center()
        .child(crate::exact::vgap(top, window))
        .child(slot.child(panel))
}

/// A rounded card with a 1px border over its background, as WebKit paints one (see
/// [`inset_background`]). Children go after the background.
pub fn card(bg: Color, border: Color, radius: f32) -> Div {
    div()
        .relative()
        .border(px(1.))
        .border_color(Hsla::from(border))
        .rounded(px(radius))
        .child(inset_background(bg, radius, 1., border))
}

/// `.nt-modal`'s shadow, for [`crate::shadow::with_shadow`] around [`modal_panel`].
pub const MODAL_SHADOW: [CssShadow; 1] = [css_box_shadow(0., 32., 90., -20., rgba(0, 0, 0, 0.8))];
/// `.palette`'s shadow.
pub const PALETTE_SHADOW: [CssShadow; 1] =
    [css_box_shadow(0., 28., 80., -18., rgba(0, 0, 0, 0.75))];
/// The editor state behind a [`text_field`] or [`input_box`].
pub fn text_input(
    placeholder: &SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.clone();
    gpui_kit::AppContext::new(cx, |cx| {
        InputState::new(window, cx).placeholder(placeholder)
    })
}

/// WebKit's default `::placeholder` color, for inputs whose class does not style it.
pub const UA_PLACEHOLDER: Color = crate::color::hex(0xa9a9a9);

/// A text `<input>` in `family` at `size` px: gpui-base's editor draws the typed text and caret
/// at `line-height: normal`; the placeholder is recur-ui text in `placeholder_color` (GPUI's own
/// is hidden by [`style_text_input`]). Fills the row it sits in.
pub fn text_field(
    input: &Entity<InputState>,
    placeholder: &SharedString,
    placeholder_color: Color,
    family: Family,
    size: f32,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let (_, _, line) = text("x")
        .family(family.clone())
        .size(size)
        .line_height(LineHeight::Normal)
        .measure(None, window);
    let state = input.read(cx);
    let placeholder = state.value().is_empty().then(|| placeholder.clone());
    // The typed value is recur-ui text where the placeholder sits, as WebKit's inner editor
    // places both; gpui-base keeps the caret and selection (its glyphs are hidden).
    let value = cx
        .try_global::<FieldLooks>()
        .and_then(|c| c.0.get(&input.entity_id()).copied())
        .filter(|l| !state.value().is_empty() && !l.masked)
        .and_then(|l| {
            Some((
                state.value().clone(),
                l.fg?,
                l.align,
                state.scroll_offset().x,
            ))
        });
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(line.height))
        .font_family(
            family
                .font(weight::REGULAR, false, Default::default())
                .family,
        )
        .text_size(px(size))
        .line_height(px(line.height))
        .text_color(t.text)
        .child(Input::new(input))
        .when_some(placeholder, |d, p| {
            d.child(
                div().absolute().top_0().left_0().child(
                    text(p)
                        .family(family.clone())
                        .size(size)
                        .line_height(LineHeight::Normal)
                        .color(placeholder_color)
                        .nowrap()
                        .field(),
                ),
            )
        })
        .when_some(value, |d, (v, fg, align, scroll_x)| {
            let at = div().absolute().top_0();
            let at = match align {
                gpui_kit::TextAlign::Left => at.left(scroll_x),
                gpui_kit::TextAlign::Right => at.right(-scroll_x),
                gpui_kit::TextAlign::Center => at.left_0().right_0().flex().justify_center(),
            };
            d.child(
                div().absolute().inset_0().overflow_hidden().child(
                    at.child(
                        text(v)
                            .family(family)
                            .size(size)
                            .line_height(LineHeight::Normal)
                            .color(fg)
                            .nowrap()
                            .field(),
                    ),
                ),
            )
        })
}

/// How [`text_field`] draws each input's value: the color [`style_text_input_fg`] gave it, and
/// what gpui-base keeps private (masking, alignment; [`set_field_shape`]).
#[derive(Clone, Copy)]
struct FieldLook {
    fg: Option<Color>,
    masked: bool,
    align: gpui_kit::TextAlign,
}

impl Default for FieldLook {
    fn default() -> Self {
        FieldLook {
            fg: None,
            masked: false,
            align: gpui_kit::TextAlign::Left,
        }
    }
}

#[derive(Default)]
struct FieldLooks(std::collections::HashMap<gpui_kit::EntityId, FieldLook>);

impl gpui_kit::Global for FieldLooks {}

fn update_field_look(input: &Entity<InputState>, cx: &mut App, f: impl FnOnce(&mut FieldLook)) {
    let id = input.entity_id();
    let looks = &mut cx.default_global::<FieldLooks>().0;
    let fresh = !looks.contains_key(&id);
    f(looks.entry(id).or_default());
    if fresh {
        cx.observe_release(input, move |_, cx| {
            if cx.has_global::<FieldLooks>() {
                cx.global_mut::<FieldLooks>().0.remove(&id);
            }
        })
        .detach();
    }
}

/// Tell [`text_field`] how `input` is masked and aligned, which gpui-base does not expose.
pub fn set_field_shape(
    input: &Entity<InputState>,
    masked: bool,
    align: gpui_kit::TextAlign,
    cx: &mut App,
) {
    update_field_look(input, cx, |l| {
        l.masked = masked;
        l.align = align;
    });
}

/// `.inp`: the bordered settings field, mono 12px, `--pane` on `--line` (amber 50% while
/// focused), radius 6, padding 9px 12px. Read-only fields mute their text and keep the border.
pub fn input_box(
    input: &Entity<InputState>,
    placeholder: &SharedString,
    readonly: bool,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    let focused = gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
    let border = if focused && !readonly {
        rgba(242, 179, 61, 0.5)
    } else {
        t.line
    };
    card(t.pane, border, 6.)
        .w_full()
        .flex()
        .py(px(9.))
        .px(px(12.))
        .font_family(MONO)
        .text_color(if readonly { t.muted } else { t.text })
        .child(text_field(
            input,
            placeholder,
            UA_PLACEHOLDER,
            Family::Mono,
            12.,
            window,
            cx,
        ))
}

/// Colors a [`text_field`]'s editor; call each render, as themes change. gpui-base reads a fully
/// transparent color as unset, so what is hidden (its placeholder; the caret in a parity capture,
/// which hides carets) is nearly transparent instead.
pub fn style_text_input(input: &Entity<InputState>, cx: &mut App) {
    let t = tokens(cx);
    style_text_input_fg(input, t.text, cx);
}

/// [`style_text_input`] with the typed text in `fg` (a field whose color follows its state).
pub fn style_text_input_fg(input: &Entity<InputState>, fg: Color, cx: &mut App) {
    style_field_editor(input, fg, true, cx);
}

/// [`style_text_input`] for a field on a translucent layer (the terminal dock's frost), where
/// WebKit draws the value without font smoothing: gpui-base's own glyphs, which are unsmoothed.
pub fn style_text_input_unsmoothed(input: &Entity<InputState>, cx: &mut App) {
    let fg = tokens(cx).text;
    style_field_editor(input, fg, false, cx);
}

fn style_field_editor(input: &Entity<InputState>, fg: Color, overlay: bool, cx: &mut App) {
    let t = tokens(cx);
    let hidden: Hsla = t.text.with_alpha(0.001).into();
    let mut masked = false;
    update_field_look(input, cx, |l| {
        l.fg = overlay.then_some(fg);
        masked = l.masked;
    });
    // Masked text stays gpui-base's (bullets); `text_field` draws everything else.
    let shown = if masked || !overlay {
        fg.into()
    } else {
        hidden
    };
    let caret = if crate::motion::is_frozen(cx) {
        hidden
    } else {
        t.text.into()
    };
    input.update(cx, |s, _| {
        s.set_editor_style(InputEditorStyle {
            foreground: shown,
            muted_foreground: hidden,
            background: Hsla::transparent_black(),
            border: Hsla::transparent_black(),
            selection: t.amber.fade(0.3).into(),
            caret,
            ..Default::default()
        })
    });
}

/// Colors a `<textarea>` (gpui-base's own placeholder in `placeholder`); the caret hides in a
/// parity capture as in [`style_text_input`].
pub fn style_textarea(
    area: &Entity<gpui_kit::base::input::TextareaState>,
    placeholder: Color,
    cx: &mut App,
) {
    let t = tokens(cx);
    let caret: Hsla = if crate::motion::is_frozen(cx) {
        t.text.with_alpha(0.001).into()
    } else {
        t.text.into()
    };
    area.update(cx, |s, _| {
        s.set_editor_style(InputEditorStyle {
            foreground: t.text.into(),
            muted_foreground: placeholder.into(),
            background: Hsla::transparent_black(),
            border: Hsla::transparent_black(),
            selection: t.amber.fade(0.3).into(),
            caret,
            ..Default::default()
        })
    });
}

/// `.nt-tip-pop`'s shadow.
pub const POPOVER_SHADOW: [CssShadow; 1] = [css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))];
/// `.emoji-suggest`'s shadow.
pub const SUGGEST_SHADOW: [CssShadow; 1] = [css_box_shadow(0., 14., 36., -12., rgba(0, 0, 0, 0.7))];
/// `.sel-menu`'s shadow.
pub const MENU_SHADOW: [CssShadow; 1] = [css_box_shadow(0., 10., 28., 0., rgba(0, 0, 0, 0.45))];

/// `.nt-modal`: 640px card, `--line-bright` border, radius 16. Wrap it in
/// [`crate::shadow::with_shadow`] with [`MODAL_SHADOW`] (and size the wrapper).
pub fn modal_panel(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane, t.line_bright, 16.).flex().flex_col()
}

/// `.palette`: the command palette card, radius 13, at most 64vh tall. Wrap it in
/// [`crate::shadow::with_shadow`] with [`PALETTE_SHADOW`], the wrapper
/// [`palette_width`] wide.
pub fn palette_panel(window: &Window, cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane, t.line_bright, 13.)
        .w_full()
        .max_h(px(f32::from(window.viewport_size().height) * 0.64))
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// Width of `.palette`, for its shadow wrapper: 580px, at most 92% of the window.
pub fn palette_width(window: &Window) -> f32 {
    580f32.min(f32::from(window.viewport_size().width) * 0.92)
}

/// `.cmd-palette`: the denser Run Command card, radius 10 (480px wide).
pub fn command_palette_panel(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane, t.line_bright, 10.)
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// `.pal-in`: the palette's input row (14px 16px, gap 11, bottom border).
pub fn palette_input_row(dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (py, pxx, gap) = if dense {
        (10., 12., 9.)
    } else {
        (14., 16., 11.)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(gap))
        .py(px(py))
        .px(px(pxx))
        .border_b(px(1.))
        .border_color(Hsla::from(t.line))
}

/// `.pal-in input`: the palette's 14.5px query field. It resets only the border, so WebKit's
/// default 1px input padding remains.
pub fn palette_input(
    input: &Entity<InputState>,
    placeholder: &SharedString,
    window: &Window,
    cx: &App,
) -> Div {
    let t = tokens(cx);
    div().flex_1().min_w_0().flex().p(px(1.)).child(text_field(
        input,
        placeholder,
        t.faint,
        Family::Ui,
        14.5,
        window,
        cx,
    ))
}

/// `.pal-g`: mono 9px uppercase group label with .12em tracking.
pub fn palette_group(label: impl Into<gpui_kit::SharedString>, dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (pad, size, track) = if dense {
        ((6., 8., 2.), 8.5, 0.14)
    } else {
        ((10., 10., 5.), 9., 0.12)
    };
    div().pt(px(pad.0)).px(px(pad.1)).pb(px(pad.2)).child(
        text(label)
            .mono()
            .size(size)
            .tracking(track)
            .uppercase()
            .color(t.faint)
            .nowrap(),
    )
}

/// `.pal-row`: palette row, `--pane-2` when selected.
pub fn palette_row(
    id: impl Into<ElementId>,
    selected: bool,
    dense: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let mut d = div().id(id).flex().items_center().cursor_pointer();
    d = if dense {
        d.py(px(3.)).px(px(8.)).rounded(px(5.)).min_h(px(24.))
    } else {
        d.gap(px(11.)).py(px(8.)).px(px(10.)).rounded(px(7.))
    };
    if selected {
        d = d.bg(t.pane_2);
    }
    d
}

/// `.pal-row .pic`: the row's 24px leading tile; its icon is `--muted`, `--amber` when selected.
pub fn palette_pic(name: crate::icon_name::IconName, selected: bool, cx: &App) -> Div {
    let t = tokens(cx);
    palette_tile(selected, cx).child(crate::icon::icon(name).size(px(14.)).color(if selected {
        t.amber
    } else {
        t.muted
    }))
}

/// `.pal-row .pic` holding something else: a `.dot` ([`palette_dot`]) or a project picture
/// (`PicBox::PALETTE`).
pub fn palette_tile(selected: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .flex_none()
        .size(px(24.))
        .rounded(px(6.))
        .bg(if selected {
            rgba(242, 179, 61, 0.15)
        } else {
            t.pane_2
        })
        .flex()
        .items_center()
        .justify_center()
}

/// `.pal-row .pic .dot`: a 7px status dot.
pub fn palette_dot(color: Color) -> Div {
    div().size(px(7.)).rounded_full().bg(color)
}

/// `.pal-row .pl`: the row label, 13px `--text`, taking the free width.
pub fn palette_label(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .child(text(label).size(13.).color(tokens(cx).text).nowrap())
}

/// `.pal-row .psub`: mono 10px `--faint` context after the label.
pub fn palette_sub(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Div {
    div().flex_none().ml(px(8.)).child(
        text(label)
            .mono()
            .size(10.)
            .color(tokens(cx).faint)
            .nowrap(),
    )
}

/// `.pal-row .pk` / `.sc-row .pk`: a keycap chip on a palette row.
pub fn palette_key(label: impl Into<gpui_kit::SharedString>, selected: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (bg, border, fg) = if selected {
        (t.pane, t.line_bright, t.text)
    } else {
        (t.pane_2, t.line, t.muted)
    };
    div()
        .ml_auto()
        .flex_none()
        .bg(bg)
        .border(px(1.))
        .border_color(Hsla::from(border))
        .rounded(px(4.))
        .py(px(2.))
        .px(px(6.))
        .child(crate::kbd::kbd(label).size(12.).color(fg))
}

/// `.pal-empty`.
pub fn palette_empty(label: impl Into<gpui_kit::SharedString>, dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div().p(px(if dense { 22. } else { 34. })).child(
        text(label)
            .mono()
            .size(if dense { 11. } else { 12. })
            .color(t.faint)
            .align(crate::text::Align::Center),
    )
}

/// `.pal-foot`: the hint footer on `--rail`, mono 9.5px.
pub fn palette_footer(dense: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (py, pxx, gap) = if dense {
        (6., 12., 14.)
    } else {
        (8., 14., 16.)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(gap))
        .py(px(py))
        .px(px(pxx))
        .border_t(px(1.))
        .border_color(Hsla::from(t.line))
        .bg(t.rail)
        .font_family(MONO)
        .text_size(px(9.5))
        .text_color(t.faint)
}

/// `.pal-foot .k`: an inline keycap span inside the palette footer (bordered inline box,
/// 5px right margin), followed in the same text by its label.
pub fn footer_key(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Span {
    let t = tokens(cx);
    span(label)
        .system()
        .weight(weight::MEDIUM)
        .tracking(0.02)
        .boxed(InlineBox {
            margin_right: 5.,
            border: 1.,
            border_color: t.line.into(),
            pad_top: 1.,
            pad_right: 5.,
            pad_bottom: 1.,
            pad_left: 5.,
            background: Some(t.pane_2.into()),
            radius: 3.,
            ..Default::default()
        })
}

/// `.pal-in .esc` / `.nt-x .esc`: the bordered `esc` hint.
pub fn esc_key(hovered: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .flex_none()
        .border(px(1.))
        .border_color(Hsla::from(if hovered { t.line_bright } else { t.line }))
        .rounded(px(4.))
        .py(px(2.))
        .px(px(6.))
        .child(
            crate::kbd::kbd("esc")
                .size(9.5)
                .color(if hovered { t.muted } else { t.faint }),
        )
}

/// `.nt-x .esc`: an inline span in the close button, so its box is the 9.5px system font's
/// content area plus padding and border, set on the button's 11px Inter strut by baseline.
pub fn nt_esc_hint(hovered: bool, window: &Window, cx: &App) -> Div {
    use crate::fonts::Family;
    use crate::text::strut_metrics;
    let t = tokens(cx);
    let strut = strut_metrics(&Family::Ui, 11., LineHeight::Normal, window);
    let m = strut_metrics(&Family::System, 9.5, LineHeight::Normal, window);
    let content = m.ascent + m.descent;
    let top = strut.baseline - m.ascent - 3.;
    div().h(px(strut.height)).child(
        div()
            .relative()
            .top(px(top))
            .border(px(1.))
            .border_color(Hsla::from(if hovered { t.line_bright } else { t.line }))
            .rounded(px(4.))
            .py(px(2.))
            .px(px(6.))
            .child(
                crate::kbd::kbd("esc")
                    .size(9.5)
                    .line_height(LineHeight::Px(content))
                    .color(if hovered { t.muted } else { t.faint }),
            ),
    )
}

/// `.nt-tip-pop`: the hover help popover, `--pane-2` with `--line-bright`, radius 9, 280px wide
/// ([`POPOVER_SHADOW`]).
pub fn popover(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane_2, t.line_bright, 9.)
        .w(px(280.))
        .py(px(11.))
        .px(px(12.))
        .text_color(t.muted)
        .text_size(px(12.))
        .line_height(relative(1.45))
}

/// `.emoji-suggest` / `.user-mention`: the floating suggestion list.
pub fn suggest_panel(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane, t.line_bright, 10.).p(px(4.))
}

/// `.sel-menu`: the dropdown menu shell shared by Select and IconPicker ([`MENU_SHADOW`]).
pub fn menu_panel(cx: &App) -> Div {
    let t = tokens(cx);
    card(t.pane_2, t.line_bright, 8.)
        .flex()
        .flex_col()
        .overflow_hidden()
        .p(px(4.))
}

/// `.prog` / `.prog-fill`: a 4px progress bar.
pub fn progress(fraction: f32, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .mt(px(10.))
        .h(px(4.))
        .rounded(px(2.))
        .bg(t.line)
        .overflow_hidden()
        // `overflow: hidden` on the 2px-round track clips the fill's ends; GPUI masks are
        // rectangular, so the fill carries the track's radius itself.
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0., 1.)))
                .rounded_l(px(2.))
                .when(fraction >= 1., |d| d.rounded_r(px(2.)))
                .bg(t.amber),
        )
}

/// `.hint`: centered mono footnote.
pub fn hint(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Div {
    div().pt(px(22.)).pb(px(30.)).child(
        text(label)
            .mono()
            .size(10.5)
            .color(tokens(cx).faint)
            .align(crate::text::Align::Center),
    )
}

/// Count chip (`.nav .ct`, `.tproj-h .ct`, …): coral with dark ink when it needs attention,
/// `--pane-2` / `--faint` when it is a plain or zero count.
pub fn count_chip(count: u64, attention: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (bg, fg) = if attention {
        (t.coral, crate::color::hex(0x0a0c10))
    } else {
        (t.pane_2, t.faint)
    };
    div().flex_none().rounded(px(10.)).px(px(6.)).bg(bg).child(
        text(count.to_string())
            .weight(weight::SEMIBOLD)
            .color(fg)
            .nowrap(),
    )
}

/// `.skel`: a loading placeholder with the shimmer sweep (`::after`, 1.4s, `translateX`
/// from -100% to 100%). Every skeleton sweeps in step, from the first one drawn.
pub fn skeleton(window: &mut Window, cx: &App) -> Div {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = tokens(cx);
    let skel = div()
        .relative()
        .overflow_hidden()
        .bg(t.pane_2)
        .rounded(px(5.))
        .min_h(px(12.));
    if crate::motion::reduced(cx) {
        return skel;
    }
    if !crate::motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    let period = 1.4;
    let elapsed = crate::motion::elapsed(*START.get_or_init(std::time::Instant::now), cx);
    let p = (elapsed.as_secs_f32() % period) / period;
    // `ease`, the animation's default timing function.
    let x = -1. + 2. * crate::motion::Easing::EASE.apply(p);
    let sheen = if Theme::of(cx).id.is_light() {
        rgba(0, 0, 0, 0.06)
    } else {
        rgba(255, 255, 255, 0.045)
    };
    // Gradients here take two stops, so each half of the three-stop sweep is its own box.
    let half = |from: Color, to: Color| {
        div()
            .relative()
            .h_full()
            .w(relative(0.5))
            .child(gradient_fill(90., (from, 0.), (to, 1.)))
    };
    skel.child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(x))
            .w_full()
            .flex()
            .child(half(sheen.fade(0.), sheen))
            .child(half(sheen, sheen.fade(0.))),
    )
}

/// `.skelrow`.
pub fn skeleton_row(cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(12.))
        .px(px(16.))
        .border_b(px(1.))
        .border_color(Hsla::from(tokens(cx).line_soft))
}

/// The solid runs `(start, length)` of a horizontal `border-style: dashed` side `len` long
/// and `thickness` thick, as WebKit's `GraphicsContextCG::drawLine` strokes it: solid corner
/// squares at both ends, then dashes and gaps one pattern width long, phased so the pattern
/// balances between the corners.
pub fn dashed_runs(len: f32, thickness: f32) -> Vec<(f32, f32)> {
    let corner = (2. * thickness).min(thickness.max(len / 3.));
    let pattern = (3. * thickness).min(thickness.max(len / 3.));
    let inner = len - 2. * corner;
    if inner <= 0. || pattern <= 0. {
        return vec![(0., len.max(0.))];
    }
    // `dashedLinePatternOffsetForPatternAndStrokeWidth`.
    let segments = (inner / pattern).floor() as i64;
    let remainder = inner - segments as f32 * pattern;
    let mut phase = pattern;
    if segments % 2 == 1 && remainder > 0. {
        phase -= remainder / 2.;
    } else if segments % 2 == 0 {
        phase += if remainder > 0. {
            phase - (pattern + remainder) / 2.
        } else {
            pattern / 2.
        };
    }
    let mut runs = vec![(0., corner)];
    // Walk the [on, off] pattern from `phase`, clipped to the inner span.
    let period = 2. * pattern;
    let mut at = -(phase % period);
    while at < inner {
        let (s, e) = (at.max(0.), (at + pattern).min(inner));
        if e > s {
            runs.push((corner + s, e - s));
        }
        at += period;
    }
    runs.push((len - corner, corner));
    runs
}

/// A dashed top border painted as WebKit does ([`dashed_runs`]): put it absolutely along the
/// top of a box that pads its top by `thickness` in place of the border.
pub fn dashed_top_border(thickness: f32, color: Color) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let len = f32::from(bounds.size.width);
            for (s, w) in dashed_runs(len, thickness) {
                let b = gpui_kit::Bounds {
                    origin: gpui_kit::point(bounds.origin.x + px(s), bounds.origin.y),
                    size: gpui_kit::size(px(w), px(thickness)),
                };
                window.paint_quad(gpui_kit::fill(b, Hsla::from(color)));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .right_0()
    .h(px(thickness))
}

#[cfg(test)]
mod dashed_tests {
    use super::dashed_runs;

    /// A 550px 1px rule: 2px corners, a 1.5px leading gap, then 3px dashes and gaps
    /// (measured in WebKit).
    #[test]
    fn dashed_rule_matches_webkit() {
        let r = dashed_runs(550., 1.);
        assert_eq!(r[0], (0., 2.));
        assert_eq!(r[1], (3.5, 3.));
        assert_eq!(r[2], (9.5, 3.));
        assert_eq!(*r.last().unwrap(), (548., 2.));
        let tail = r[r.len() - 2];
        assert_eq!(
            tail.0 + tail.1,
            546.5,
            "a 1.5px gap before the right corner"
        );
    }
}
