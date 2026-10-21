//! The app icons the editor draws (`icons.ts` entries for gutter severity
//! marks and the find / go-to-line panels), as SVG documents painted as
//! tinted masks (`currentColor`).

use gpui_kit::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Style, TransformationMatrix, Window, px,
};

/// An icon: its SVG body, stroke width and whether it is filled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconDef {
    pub name: &'static str,
    pub body: &'static str,
    pub stroke_width: Option<f32>,
    pub filled: bool,
}

const fn stroke(name: &'static str, body: &'static str) -> IconDef {
    IconDef {
        name,
        body,
        stroke_width: None,
        filled: false,
    }
}

pub const X: IconDef = stroke("x", r#"<path d="M6 6l12 12M18 6L6 18"/>"#);
pub const SEARCH: IconDef = IconDef {
    stroke_width: Some(1.75),
    ..stroke(
        "search",
        r#"<circle cx="11" cy="11" r="6"/><path d="M16 16l4 4"/>"#,
    )
};
pub const REPLACE: IconDef = stroke(
    "replace",
    r#"<path d="M4 7h11"/><path d="M12 4l3 3-3 3"/><path d="M20 17H9"/><path d="M12 14l-3 3 3 3"/>"#,
);
pub const CASE_SENSITIVE: IconDef = stroke(
    "case-sensitive",
    r#"<path d="m2 16 4.039-9.69a.5.5 0 0 1 .923 0L11 16"/><path d="M22 9v7"/><path d="M3.304 13h6.392"/><circle cx="18.5" cy="12.5" r="3.5"/>"#,
);
pub const REGEX: IconDef = stroke(
    "regex",
    r#"<path d="M17 3v10"/><path d="m12.67 5.5 8.66 5"/><path d="m12.67 10.5 8.66-5"/><path d="M9 17a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v2a2 2 0 0 0 2 2h2a2 2 0 0 0 2-2v-2z"/>"#,
);
pub const WHOLE_WORD: IconDef = stroke(
    "whole-word",
    r#"<circle cx="7" cy="12" r="3"/><path d="M10 9v6"/><circle cx="17" cy="12" r="3"/><path d="M14 7v8"/><path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/>"#,
);
pub const CHEVRON_LEFT: IconDef = stroke("chevron-left", r#"<path d="M15 18l-6-6 6-6"/>"#);
pub const CHEVRON_RIGHT: IconDef = stroke("chevron-right", r#"<path d="M9 6l6 6-6 6"/>"#);
pub const HASH: IconDef = stroke(
    "hash",
    r#"<path d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 18"/>"#,
);
pub const SEV_CRITICAL: IconDef = IconDef {
    name: "sev-critical",
    body: r#"<path d="M15.3 2h-6.6c-.562 0 -1.016 .201 -1.407 .593l-4.7 4.7a1.894 1.894 0 0 0 -.593 1.407v6.6c0 .562 .201 1.016 .593 1.407l4.7 4.7c.391 .392 .845 .593 1.407 .593h6.6c.562 0 1.016 -.201 1.407 -.593l4.7 -4.7c.392 -.391 .593 -.845 .593 -1.407v-6.6c0 -.562 -.201 -1.016 -.593 -1.407l-4.7 -4.7a1.894 1.894 0 0 0 -1.407 -.593z"/>"#,
    stroke_width: None,
    filled: true,
};
pub const SEV_HIGH: IconDef = IconDef {
    name: "sev-high",
    body: r#"<path d="M12 1.67c.955 0 1.845 .467 2.39 1.247l.105 .16l8.114 13.548a2.914 2.914 0 0 1 -2.307 4.363l-.195 .008h-16.225a2.914 2.914 0 0 1 -2.582 -4.2l.099 -.185l8.11 -13.538a2.914 2.914 0 0 1 2.491 -1.403zm.01 13.33l-.127 .007a1 1 0 0 0 0 1.986l.117 .007l.127 -.007a1 1 0 0 0 0 -1.986l-.117 -.007zm-.01 -7a1 1 0 0 0 -.993 .883l-.007 .117v4l.007 .117a1 1 0 0 0 1.986 0l.007 -.117v-4l-.007 -.117a1 1 0 0 0 -.993 -.883z"/>"#,
    stroke_width: None,
    filled: true,
};
pub const SEV_MEDIUM: IconDef = IconDef {
    name: "sev-medium",
    body: r#"<path d="M12 9v4"/><path d="M10.363 3.591l-8.106 13.534a1.914 1.914 0 0 0 1.636 2.871h16.214a1.914 1.914 0 0 0 1.636 -2.87l-8.106 -13.536a1.914 1.914 0 0 0 -3.274 0"/><path d="M12 16h.01"/>"#,
    stroke_width: Some(2.0),
    filled: false,
};
pub const SEV_LOW: IconDef = IconDef {
    name: "sev-low",
    body: r#"<path d="M3 12a9 9 0 1 0 18 0a9 9 0 0 0 -18 0"/><path d="M12 8v4"/><path d="M12 16h.01"/>"#,
    stroke_width: Some(2.0),
    filled: false,
};
pub const SEV_INFO: IconDef = IconDef {
    name: "sev-info",
    body: r#"<path d="M12 2c5.523 0 10 4.477 10 10a10 10 0 0 1 -19.995 .324l-.005 -.324l.004 -.28c.148 -5.393 4.566 -9.72 9.996 -9.72zm0 9h-1l-.117 .007a1 1 0 0 0 0 1.986l.117 .007v3l.007 .117a1 1 0 0 0 .876 .876l.117 .007h1l.117 -.007a1 1 0 0 0 .876 -.876l.007 -.117l-.007 -.117a1 1 0 0 0 -.764 -.857l-.112 -.02l-.117 -.006v-3l-.007 -.117a1 1 0 0 0 -.876 -.876l-.117 -.007zm.01 -3l-.127 .007a1 1 0 0 0 0 1.986l.117 .007l.127 -.007a1 1 0 0 0 0 -1.986l-.117 -.007z"/>"#,
    stroke_width: None,
    filled: true,
};

/// Icon by `icons.ts` name.
pub fn by_name(name: &str) -> Option<IconDef> {
    Some(match name {
        "x" => X,
        "search" => SEARCH,
        "replace" => REPLACE,
        "case-sensitive" => CASE_SENSITIVE,
        "regex" => REGEX,
        "whole-word" => WHOLE_WORD,
        "chevron-left" => CHEVRON_LEFT,
        "chevron-right" => CHEVRON_RIGHT,
        "hash" => HASH,
        "sev-critical" => SEV_CRITICAL,
        "sev-high" => SEV_HIGH,
        "sev-medium" => SEV_MEDIUM,
        "sev-low" => SEV_LOW,
        "sev-info" => SEV_INFO,
        _ => return None,
    })
}

/// Lint severity -> icon (`lintSevIcon`).
pub fn lint_icon(severity: crate::lint::Severity) -> IconDef {
    use crate::lint::Severity::*;
    match severity {
        Error => SEV_HIGH,
        Warning => SEV_MEDIUM,
        Info => SEV_INFO,
        Hint => SEV_LOW,
    }
}

impl IconDef {
    /// The SVG document Icon.svelte builds (black paint; tinted when drawn).
    pub fn svg(&self) -> String {
        let paint = if self.filled {
            r#"fill="black""#.to_string()
        } else {
            format!(
                r#"fill="none" stroke="black" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round""#,
                self.stroke_width.unwrap_or(2.0)
            )
        };
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {paint}>{}</svg>"#,
            self.body
        )
    }

    /// Paint into `bounds` in `color`.
    pub fn paint(&self, bounds: Bounds<Pixels>, color: Hsla, window: &mut Window, cx: &App) {
        let svg = self.svg();
        let key = format!("recur-editor-icon://{}", self.name);
        let _ = window.paint_svg(
            bounds,
            key.into(),
            Some(svg.as_bytes()),
            TransformationMatrix::unit(),
            color,
            cx,
        );
    }

    /// A `size`-pixel icon element drawn in the inherited text color
    /// (`currentColor`), so hover and pressed states recolor it.
    pub fn element(self, size: f32) -> IconElement {
        IconElement {
            icon: self,
            size: px(size),
        }
    }
}

pub struct IconElement {
    icon: IconDef,
    size: Pixels,
}

impl IntoElement for IconElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for IconElement {
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
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = self.size.into();
        style.size.height = self.size.into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let color = window.text_style().color;
        self.icon.paint(bounds, color, window, cx);
    }
}
