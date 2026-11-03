//! Overlapping participant faces (review state as a ring and a corner
//! mark, `+N` past the cap) and, on hover, the popout naming everyone with their review state.

use std::sync::LazyLock;

use crate::avatar::{Fill, parse_avatar_css};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, px,
};
use recur_domain::Pr;
use recur_forge::Forge;
use recur_forge::github::GitHub;
use recur_forge::view::{
    PrPerson, participant_avatar_url, participant_initials, participant_label, person_key,
    pr_face_slots, pr_people_with_reviews, review_state_label,
};
use recur_ui::fonts::Family;
use recur_ui::shadow::{box_shadow, css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, Theme, Tokens};

use recur_ui::set::hsla;

static GITHUB: LazyLock<GitHub> = LazyLock::new(GitHub::default);

const FACE: f32 = 20.;
/// `.pr-face { margin-left: -7px }`.
const OVERLAP: f32 = 7.;
const MARK: f32 = 9.;

fn review_color(state: &str, t: &Tokens) -> Option<Color> {
    match state {
        "approved" => Some(t.emerald),
        "changes_requested" => Some(t.coral),
        "pending" => Some(t.amber),
        _ => None,
    }
}

fn mark_glyph(state: &str) -> &'static str {
    match state {
        "approved" => "✓",
        "changes_requested" => "✕",
        _ => "",
    }
}

/// The face's fill and ink from its inline `avatarCss` over `.pr-face`'s defaults.
fn face_paint(p: &PrPerson, more: bool, cx: &App) -> (Option<Fill>, Color) {
    let t = Theme::of(cx).tokens;
    let (fill, color) = if more || p.avatar_css.is_empty() {
        (None, None)
    } else {
        parse_avatar_css(&p.avatar_css, cx)
    };
    let ink = color.unwrap_or(if more { t.faint } else { t.muted });
    (fill.or(Some(Fill::Solid(t.pane_2))), ink)
}

/// `.pr-face`: a 20px circle with mono initials and the photo over them. `ring` is the
/// `box-shadow: 0 0 0 1.5px` color (none in the popout).
fn face(p: &PrPerson, label: Option<String>, ring: Option<Color>, cx: &App) -> Div {
    let more = label.is_some();
    let (fill, ink) = face_paint(p, more, cx);
    let mut body = div()
        .relative()
        .size(px(FACE))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center();
    match fill {
        Some(Fill::Solid(c)) => body = body.bg(hsla(c)),
        Some(Fill::Gradient(angle, a, b)) => {
            body = body.child(recur_ui::style::css_gradient(
                angle,
                (a, 0.),
                (b, 1.),
                FACE / 2.,
                0.,
                recur_ui::color::TRANSPARENT,
            ))
        }
        None => {}
    }
    let initials = label.unwrap_or_else(|| participant_initials(p));
    let (size, weight) = if more { (8.5, 650.) } else { (8., 700.) };
    body = body.child(
        text(initials)
            .family(Family::Mono)
            .size(size)
            .weight(weight)
            .line_height(LineHeight::Scale(1.))
            .color(ink)
            .nowrap(),
    );
    let url = if more {
        String::new()
    } else {
        participant_avatar_url(p, &*GITHUB as &dyn Forge)
    };
    if !url.is_empty() {
        body = body.child(
            img(SharedString::from(url))
                .absolute()
                .inset_0()
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    match ring {
        Some(c) => with_shadow(body, &[css_box_shadow(0., 0., 0., 1.5, c)], FACE / 2.).flex_none(),
        None => body.flex_none(),
    }
}

/// `.pr-face-mark` for a review state, ringed in `ring`.
fn mark(state: &str, ring: Color, cx: &App) -> Option<Div> {
    let t = Theme::of(cx).tokens;
    let bg = review_color(state, &t)?;
    Some(
        div()
            .relative()
            .size(px(MARK))
            .rounded_full()
            .bg(hsla(bg))
            .flex()
            .items_center()
            .justify_center()
            .child(box_shadow(
                &[css_box_shadow(0., 0., 0., 1.5, ring)],
                MARK / 2.,
                0.,
            ))
            .child(
                text(mark_glyph(state))
                    .family(Family::Mono)
                    .size(7.)
                    .weight(800.)
                    .line_height(LineHeight::Scale(1.))
                    .color(t.pane)
                    .nowrap(),
            ),
    )
}

/// `.pr-people-pop`: everyone on the PR with their review state. The host places it 6px
/// below the stack (`top: calc(100% + 6px); left: 0`) in a layer above its rows.
pub fn popout_for(key: &str, people: &[PrPerson], cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let rows = people.iter().map(|p| {
        let state = p.review_state.as_str();
        let rev = review_state_label(state);
        let mut f = face(p, None, None, cx);
        if let Some(m) = mark(state, t.rail, cx) {
            f = f.child(div().absolute().right(px(-1.)).bottom(px(-1.)).child(m));
        }
        let mut meta = div().flex().flex_col().min_w_0().child(
            text(participant_label(p))
                .ui()
                .size(12.)
                .color(t.text)
                .truncate(),
        );
        if !rev.is_empty() {
            let c = review_color(state, &t).unwrap_or(t.muted);
            meta = meta.child(div().mt(px(1.)).child(text(rev).ui().size(11.).color(c)));
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .py(px(4.))
            .px(px(6.))
            .rounded(px(5.))
            .child(f)
            .child(meta)
    });
    let panel = recur_ui::style::card(t.rail, t.line, 8.)
        .min_w(px(156.))
        .max_w(px(220.))
        .p(px(6.))
        .font_family(Family::Ui.primary())
        .children(rows);
    let sel = format!("pr-people-pop:{key}");
    div().debug_selector(move || sel.clone()).child(with_shadow(
        panel,
        &[css_box_shadow(
            0.,
            6.,
            18.,
            0.,
            recur_ui::rgba(0, 0, 0, 0.3),
        )],
        8.,
    ))
}

/// The stack for `people`. `ring` is the faces' gap ring (`--pane`, `--pane-2` under a
/// hovered list row); `on_hover` reports the pointer so the host can show [`popout_for`].
pub fn people_stack(
    key: &str,
    people: &[PrPerson],
    ring: Color,
    on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let root = div()
        .id(SharedString::from(format!("pr-people-{key}")))
        .debug_selector({
            let k = key.to_string();
            move || format!("pr-people:{k}")
        })
        .relative()
        .flex()
        .items_center()
        .flex_none()
        .min_w_0()
        .min_h(px(20.))
        .on_hover(on_hover);
    if people.is_empty() {
        return root.into_any_element();
    }
    let (shown, extra, _) = pr_face_slots(people);
    // The faces overlap by 7px; Taffy's max-content ignores negative margins, so the stack's
    // width is set from the face count.
    let count = shown.len() + usize::from(extra > 0);
    let width = FACE + (count.saturating_sub(1)) as f32 * (FACE - OVERLAP);
    let mut faces = div().relative().flex().flex_none().w(px(width));
    let mut marks: Vec<AnyElement> = Vec::new();
    for (i, p) in shown.iter().enumerate() {
        let state = p.review_state.as_str();
        let ring_c = review_color(state, &t).unwrap_or(ring);
        let k = person_key(p);
        let f = face(p, None, Some(ring_c), cx)
            .when(i > 0, |d| d.ml(px(-OVERLAP)))
            .debug_selector(move || format!("pr-face:{k}"));
        faces = faces.child(f);
        // `z-index: 2` marks paint over every face of the stack, so they go after them.
        if let Some(m) = mark(state, t.pane, cx) {
            let left = i as f32 * (FACE - OVERLAP) + FACE + 1. - MARK;
            marks.push(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(FACE + 1. - MARK))
                    .child(m)
                    .into_any_element(),
            );
        }
    }
    if extra > 0 {
        let more = PrPerson::default();
        faces = faces.child(
            face(&more, Some(format!("+{extra}")), Some(ring), cx)
                .ml(px(-OVERLAP))
                .debug_selector(move || format!("pr-face-more=+{extra}")),
        );
    }
    faces = faces.children(marks);
    root.child(faces).into_any_element()
}

/// The people on a PR, review states merged in (`prPeopleWithReviews`).
pub fn pr_people(pr: &Pr) -> Vec<PrPerson> {
    pr_people_with_reviews(pr)
}
