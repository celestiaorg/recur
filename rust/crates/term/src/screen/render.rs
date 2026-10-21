//! The viewport as xterm.js's WebGL renderer resolves it: `CellColorResolver` (selection, search
//! decorations, inverse), the cursor cell override, background rectangles
//! (`RectangleRenderer`) and glyph colours (`TextureAtlas._getForegroundColor`).

use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use super::buffer::Buffer;
use super::overline;
use super::theme::Rgba;
use super::{CursorStyle, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Underline {
    pub style: UnderlineStyle,
    pub color: Rgba,
}

/// One cell's glyph. `width` is 2 for wide characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub col: usize,
    pub row: usize,
    pub text: String,
    pub width: usize,
    pub fg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<Underline>,
    pub strikethrough: bool,
    /// SGR 53, drawn along the top of the character box in the glyph colour.
    pub overline: bool,
    /// Box drawing, block and powerline glyphs (drawn procedurally to fill the cell).
    pub codepoint: u32,
}

/// An opaque background run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgRect {
    pub col: usize,
    pub row: usize,
    pub len: usize,
    pub color: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Bar,
    Underline,
    Outline,
}

/// A non-block cursor (a block cursor is resolved into its cell's colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorDraw {
    pub col: usize,
    pub row: usize,
    pub width: usize,
    pub shape: CursorShape,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    /// The viewport clear colour (theme background, possibly translucent).
    pub background: Rgba,
    pub rects: Vec<BgRect>,
    pub glyphs: Vec<Glyph>,
    pub cursor: Option<CursorDraw>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Default,
    Palette(u8),
    Rgb(Rgba),
}

#[derive(Clone, Copy, Debug)]
struct Attrs {
    fg: Mode,
    bg: Mode,
    inverse: bool,
    bold: bool,
    italic: bool,
    dim: bool,
    invisible: bool,
    strike: bool,
    /// xterm's `BgFlags.OVERLINE`.
    overline: bool,
    underline: Option<UnderlineStyle>,
    underline_color: Option<Mode>,
    /// xterm's `BgFlags.HAS_EXTENDED` (underline style/colour or an OSC 8 link).
    extended: bool,
}

fn mode(c: Color, default: NamedColor) -> Mode {
    match c {
        Color::Spec(rgb) => Mode::Rgb(Rgba::rgb(rgb.r, rgb.g, rgb.b)),
        Color::Indexed(i) => Mode::Palette(i),
        Color::Named(n) if n == default => Mode::Default,
        Color::Named(n) if (n as usize) < 16 => Mode::Palette(n as u8),
        Color::Named(_) => Mode::Default,
    }
}

fn attrs(cell: &Cell) -> Attrs {
    let f = cell.flags;
    let underline = if f.contains(Flags::DOUBLE_UNDERLINE) {
        Some(UnderlineStyle::Double)
    } else if f.contains(Flags::UNDERCURL) {
        Some(UnderlineStyle::Curly)
    } else if f.contains(Flags::DOTTED_UNDERLINE) {
        Some(UnderlineStyle::Dotted)
    } else if f.contains(Flags::DASHED_UNDERLINE) {
        Some(UnderlineStyle::Dashed)
    } else if f.contains(Flags::UNDERLINE) {
        Some(UnderlineStyle::Single)
    } else {
        None
    };
    let underline_color = cell
        .underline_color()
        .map(|c| mode(c, NamedColor::Foreground));
    Attrs {
        fg: mode(cell.fg, NamedColor::Foreground),
        bg: mode(cell.bg, NamedColor::Background),
        inverse: f.contains(Flags::INVERSE),
        bold: f.contains(Flags::BOLD),
        italic: f.contains(Flags::ITALIC),
        dim: f.contains(Flags::DIM),
        invisible: f.contains(Flags::HIDDEN),
        strike: f.contains(Flags::STRIKEOUT),
        overline: overline::is_overlined(cell),
        underline,
        underline_color,
        extended: underline.is_some() || underline_color.is_some() || cell.hyperlink().is_some(),
    }
}

struct Colors {
    fg: Rgba,
    bg: Rgba,
    cursor: Rgba,
    cursor_accent: Rgba,
    ansi: [Rgba; 256],
    sel_opaque: Rgba,
    sel_inactive_opaque: Rgba,
    sel_fg: Option<Rgba>,
}

impl Colors {
    fn of(&self, m: Mode, default: Rgba) -> Rgba {
        match m {
            Mode::Default => default,
            Mode::Palette(i) => self.ansi[i as usize],
            Mode::Rgb(c) => c,
        }
    }
}

/// Powerline and box/block glyphs take the background's role under a selection.
fn glyph_as_background(cp: u32) -> bool {
    (0xE0A4..=0xE0D6).contains(&cp) || (0x2500..=0x259F).contains(&cp)
}

pub(super) fn frame(s: &Screen, focused: bool, cursor_blink_on: bool) -> Frame {
    let buf = Buffer::new(&s.term);
    let p = &s.palette;
    let mut ansi = p.ansi;
    for (i, c) in ansi.iter_mut().enumerate() {
        if let Some(o) = s.term.colors()[i] {
            *c = Rgba::rgb(o.r, o.g, o.b);
        }
    }
    let colors = Colors {
        fg: s.special(NamedColor::Foreground, p.foreground),
        bg: s.special(NamedColor::Background, p.background),
        cursor: s.special(NamedColor::Cursor, p.cursor),
        cursor_accent: p.cursor_accent,
        ansi,
        sel_opaque: p.selection_background_opaque,
        sel_inactive_opaque: p.selection_inactive_background_opaque,
        sel_fg: p.selection_foreground,
    };
    let sel_bg = if focused {
        colors.sel_opaque
    } else {
        colors.sel_inactive_opaque
    };
    let (cols, rows) = (s.cols(), s.rows());
    let ydisp = buf.ydisp();
    let ybase = buf.ybase();

    let (style, blinks) = s.cursor_style();
    let cursor_visible =
        s.cursor_initialized && !s.cursor_hidden() && (!blinks || !focused || cursor_blink_on);
    let cursor = s.term.grid().cursor.point;
    let cursor_row_abs = ybase as i64 + cursor.line.0 as i64;
    let cursor_x = cursor.column.0.min(cols - 1);
    let block = if focused {
        style == CursorStyle::Block
    } else {
        false
    };

    let mut frame = Frame {
        cols,
        rows,
        background: colors.bg,
        rects: Vec::new(),
        glyphs: Vec::new(),
        cursor: None,
    };
    for y in 0..rows {
        let row = ydisp + y;
        let line = Line(row as i32 - ybase as i32);
        let grid_row = &s.term.grid()[line];
        let mut last_cursor_x: Option<usize> = None;
        for x in 0..cols {
            let cell = &grid_row[Column(x)];
            let mut a = attrs(cell);
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let null = cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
            let cp = if null { 0 } else { u32::from(cell.c) };

            // CellColorResolver.resolve
            let (mut ov_bg, mut ov_fg): (Option<Rgba>, Option<Rgba>) = (None, None);
            let (bottom, top) = s.search.highlight_at(&buf, x as i64, row as i64);
            if let Some(c) = bottom {
                ov_bg = Some(c);
            }
            let selected = s.selection.is_cell_selected(&buf, x as i64, row as i64);
            if selected {
                let base = if a.inverse {
                    Some(colors.of(a.fg, colors.fg))
                } else {
                    match a.bg {
                        Mode::Default => None,
                        m => Some(colors.of(m, colors.bg)),
                    }
                };
                ov_bg = Some(match base {
                    Some(b) => Rgba::blend(b, Rgba((sel_bg.0 & 0xFFFF_FF00) | 0x80)),
                    None => sel_bg,
                });
                if let Some(c) = colors.sel_fg {
                    ov_fg = Some(c);
                }
                if glyph_as_background(cp) {
                    ov_fg = Some(if a.inverse && a.bg == Mode::Default {
                        sel_bg
                    } else {
                        let base = if a.inverse {
                            colors.of(a.bg, colors.bg)
                        } else {
                            colors.of(a.fg, colors.fg)
                        };
                        Rgba::blend(base, Rgba((sel_bg.0 & 0xFFFF_FF00) | 0x80))
                    });
                }
            }
            if let Some(c) = top {
                ov_bg = Some(c);
            }
            let orig_inverse = a.inverse;
            if let Some(c) = ov_bg {
                if selected {
                    a.dim = false;
                }
                a.bg = Mode::Rgb(c);
            }
            if let Some(c) = ov_fg {
                a.inverse = false;
                a.fg = Mode::Rgb(c);
            }
            if orig_inverse {
                if ov_bg.is_some() && ov_fg.is_none() {
                    let raw_bg = attrs(cell).bg;
                    a.fg = match raw_bg {
                        Mode::Default => Mode::Rgb(colors.bg.opaque()),
                        m => m,
                    };
                    a.inverse = false;
                }
                if ov_bg.is_none() && ov_fg.is_some() {
                    let raw_fg = attrs(cell).fg;
                    a.bg = match raw_fg {
                        Mode::Default => Mode::Rgb(colors.fg),
                        m => m,
                    };
                }
            }

            // Cursor cell override.
            let on_cursor_row = cursor_visible && row as i64 == cursor_row_abs;
            if on_cursor_row && x == cursor_x {
                last_cursor_x = Some(cursor_x + width - 1);
                if !block {
                    let shape = match (focused, style) {
                        (false, _) => CursorShape::Outline,
                        (true, CursorStyle::Bar) => CursorShape::Bar,
                        (true, CursorStyle::Underline) => CursorShape::Underline,
                        (true, CursorStyle::Block) => CursorShape::Outline,
                    };
                    frame.cursor = Some(CursorDraw {
                        col: cursor_x,
                        row: y,
                        width,
                        shape,
                        color: colors.cursor,
                    });
                }
            }
            if block && on_cursor_row && x >= cursor_x && last_cursor_x.is_some_and(|l| x <= l) {
                a = Attrs {
                    fg: Mode::Rgb(colors.cursor_accent),
                    bg: Mode::Rgb(colors.cursor),
                    inverse: false,
                    bold: false,
                    italic: false,
                    dim: false,
                    invisible: false,
                    strike: false,
                    overline: false,
                    underline: None,
                    underline_color: None,
                    extended: false,
                };
            }

            // RectangleRenderer: a rect wherever xterm's packed bg is non-zero, or inverse.
            let bg_nonzero = a.bg != Mode::Default || a.italic || a.dim || a.overline || a.extended;
            if bg_nonzero || a.inverse {
                let color = if a.inverse {
                    colors.of(a.fg, colors.fg)
                } else {
                    colors.of(a.bg, colors.bg)
                }
                .opaque();
                match frame.rects.last_mut() {
                    Some(r) if r.row == y && r.col + r.len == x && r.color == color => r.len += 1,
                    _ => frame.rects.push(BgRect {
                        col: x,
                        row: y,
                        len: 1,
                        color,
                    }),
                }
            }

            if null || a.invisible || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            // TextureAtlas._getForegroundColor
            let (fg_mode, inverse) = (if a.inverse { a.bg } else { a.fg }, a.inverse);
            let mut fg = match fg_mode {
                Mode::Palette(i) => {
                    colors.ansi[if a.bold && i < 8 {
                        i as usize + 8
                    } else {
                        i as usize
                    }]
                }
                Mode::Rgb(c) => c,
                Mode::Default => {
                    if inverse {
                        colors.bg
                    } else {
                        colors.fg
                    }
                }
            }
            .opaque();
            if a.dim {
                fg = fg.multiply_opacity(0.5);
            }
            let underline = a.underline.map(|style| Underline {
                style,
                color: match a.underline_color {
                    None | Some(Mode::Default) => fg,
                    Some(Mode::Rgb(c)) => c,
                    Some(Mode::Palette(i)) => {
                        colors.ansi[if a.bold && i < 8 {
                            i as usize + 8
                        } else {
                            i as usize
                        }]
                    }
                },
            });
            let mut text = String::new();
            text.push(cell.c);
            if let Some(zw) = cell.zerowidth() {
                text.extend(zw);
            }
            if cell.c == ' '
                && underline.is_none()
                && !a.strike
                && !a.overline
                && cell.zerowidth().is_none()
            {
                continue;
            }
            frame.glyphs.push(Glyph {
                col: x,
                row: y,
                text,
                width,
                fg,
                bold: a.bold,
                italic: a.italic,
                underline,
                strikethrough: a.strike,
                overline: a.overline,
                codepoint: cp,
            });
        }
    }
    frame
}
