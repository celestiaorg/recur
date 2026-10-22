//! The markdown stylesheets: `.chat-canvas` (`_chat.scss`) and `.file-preview-md .md-doc`
//! (`_observatory.scss`), with the UA defaults they leave in place. Lengths are CSS px at
//! the element's font size; [`lu`] truncates them to WebKit LayoutUnits.

use recur_ui::{Color, Tokens};

pub use recur_ui::exact::lu;

/// Which host class the markdown renders under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkdownClass {
    /// `.chat-canvas` (agent chat, findings, PR bodies, plans).
    ChatCanvas,
    /// `.md-doc` inside `.file-preview-md` (whole-file markdown preview).
    MdDoc,
}

/// Per-heading values: font size (em of the body), margins (em of the heading), line height.
#[derive(Clone, Copy, Debug)]
pub struct Heading {
    pub scale: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub muted: bool,
}

/// Every value the cascade gives the elements marked emits.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub class: MarkdownClass,
    pub size: f32,
    pub leading: f32,
    /// `.md-doc { max-width: 46em; margin: 0 auto }`.
    pub max_width: Option<f32>,
    pub headings: [Heading; 6],
    pub heading_leading: f32,
    /// `h1/h2/h3:first-child { margin-top }` in em of the heading.
    pub heading_first_top: Option<f32>,
    pub p_margin_bottom: f32,
    pub p_last_child_zero: bool,
    pub list_margin: (f32, f32),
    pub list_indent: f32,
    pub li_margin: f32,
    pub nested_list_margin: f32,
    pub li_p_margin_zero: bool,
    pub code_size: f32,
    pub code_colored: bool,
    pub pre_margin: (f32, f32),
    pub pre_padding: (f32, f32),
    pub pre_code_size: f32,
    pub mermaid_padding: (f32, f32),
    pub mermaid_rendered_padding: (f32, f32),
    pub blockquote_margin: (f32, f32),
    pub blockquote_padding_v: f32,
    pub blockquote_padding_left: f32,
    pub blockquote_p_margin: f32,
    pub hr_margin: f32,
    pub table_size: f32,
    pub table_margin: (f32, f32),
    pub cell_padding: (f32, f32),
    /// `.chat-canvas strong { font-weight: 600 }`; the doc keeps the UA's `bolder` (700).
    pub strong_weight: f32,
    /// `.chat-canvas em { color: var(--muted) }`.
    pub em_muted: bool,
    /// `.chat-canvas .mdref` card styles (the doc sheet has none).
    pub mdref_card: bool,
    /// `--text: var(--muted)` (the chat's thought and compaction bodies): text-colored runs
    /// paint muted and hovered links stay cyan.
    pub muted: bool,
    /// The body weight a host's `font: inherit` hands down (400 by default).
    pub weight: f32,
    /// `letter-spacing` in em inherited from the host (0 by default).
    pub tracking: f32,
    /// The first and last blocks' outer margins collapse into a host without padding or
    /// border whose own margins are at least as large (a chat row), instead of padding the view.
    pub collapse_edges: bool,
}

const fn h(scale: f32, margin_top: f32, margin_bottom: f32, muted: bool) -> Heading {
    Heading {
        scale,
        margin_top,
        margin_bottom,
        muted,
    }
}

impl Sheet {
    pub fn for_class(class: MarkdownClass) -> Sheet {
        match class {
            MarkdownClass::ChatCanvas => Sheet::chat(),
            MarkdownClass::MdDoc => Sheet::doc(),
        }
    }

    pub fn chat() -> Sheet {
        let hx = |s| h(s, 1.15, 0.45, false);
        Sheet {
            class: MarkdownClass::ChatCanvas,
            size: 13.,
            leading: 1.6,
            max_width: None,
            headings: [hx(1.25), hx(1.15), hx(1.05), hx(1.05), hx(1.05), hx(1.05)],
            heading_leading: 1.3,
            heading_first_top: Some(0.1),
            p_margin_bottom: 0.7,
            p_last_child_zero: true,
            list_margin: (0.2, 0.7),
            list_indent: 1.35,
            li_margin: 0.2,
            nested_list_margin: 0.2,
            li_p_margin_zero: true,
            code_size: 11.5,
            code_colored: true,
            pre_margin: (0.55, 0.85),
            pre_padding: (10., 12.),
            pre_code_size: 11.5,
            mermaid_padding: (12., 14.),
            mermaid_rendered_padding: (10., 12.),
            blockquote_margin: (0.5, 0.8),
            blockquote_padding_v: 0.1,
            blockquote_padding_left: 12.,
            blockquote_p_margin: 0.2,
            hr_margin: 1.1,
            table_size: 12.,
            table_margin: (0.55, 0.85),
            cell_padding: (5., 10.),
            strong_weight: 600.,
            em_muted: true,
            mdref_card: true,
            muted: false,
            weight: 400.,
            tracking: 0.,
            collapse_edges: false,
        }
    }

    pub fn doc() -> Sheet {
        Sheet {
            class: MarkdownClass::MdDoc,
            size: 14.,
            leading: 1.65,
            max_width: Some(46. * 14.),
            headings: [
                h(1.7, 0., 0.5, false),
                h(1.35, 1.25, 0.5, false),
                h(1.15, 1.25, 0.5, false),
                h(1., 1.25, 0.5, true),
                h(1., 1.25, 0.5, true),
                h(1., 1.25, 0.5, true),
            ],
            heading_leading: 1.25,
            heading_first_top: None,
            p_margin_bottom: 0.75,
            p_last_child_zero: false,
            list_margin: (0.2, 0.75),
            list_indent: 1.35,
            li_margin: 0.25,
            nested_list_margin: 0.2,
            li_p_margin_zero: false,
            code_size: 12.,
            code_colored: false,
            pre_margin: (0.6, 0.9),
            pre_padding: (12., 14.),
            pre_code_size: 12.,
            mermaid_padding: (12., 14.),
            mermaid_rendered_padding: (12., 14.),
            blockquote_margin: (0.5, 0.85),
            blockquote_padding_v: 0.15,
            blockquote_padding_left: 14.,
            blockquote_p_margin: 0.,
            hr_margin: 1.4,
            table_size: 13.,
            table_margin: (0.6, 0.9),
            cell_padding: (6., 12.),
            strong_weight: 700.,
            em_muted: false,
            mdref_card: false,
            muted: false,
            weight: 400.,
            tracking: 0.,
            collapse_edges: false,
        }
    }
}

/// `sh-*` token colors (`_chat.scss`, global rules shared by every theme).
pub fn sh_color(class: &str) -> Option<Color> {
    let rgb = match class {
        "sh-kw" => 0x6cb6ff,
        "sh-str" => 0x7ee08a,
        "sh-com" => 0x7fa9ad,
        "sh-flag" => 0xe0837d,
        "sh-var" | "sh-op" => 0xe5c07b,
        "sh-num" => 0xd19a66,
        _ => return None,
    };
    Some(recur_ui::hex(rgb))
}

/// `.sh-prompt { color: var(--faint) }`.
pub fn sh_prompt(t: &Tokens) -> Color {
    t.faint
}

/// `.md-copy.copied { color: #3fb950 }`.
pub fn copied_green() -> Color {
    recur_ui::hex(0x3fb950)
}
