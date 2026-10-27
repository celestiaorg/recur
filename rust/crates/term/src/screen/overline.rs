//! SGR 53 / 55 (overline on / off), which xterm.js renders and alacritty_terminal does not model,
//! and SGR 221 / 222 (kitty's not bold / not faint, which xterm.js 6 applies), which vte drops.
//!
//! The attribute lives in the cell, as a spare bit of alacritty's `Flags` set on the cursor
//! template. Printing copies the template's flags, so scrollback, reflow, the alternate screen
//! and DECSC / DECRC carry overline like any other attribute, and erased or inserted cells drop
//! it (alacritty resets them to the template's background only, as xterm.js's erase attributes
//! do). SGR 0 and RIS clear it with the rest of the template.
//!
//! vte drops SGR codes it does not know before they reach the handler, so the screen follows
//! each SGR that changes overline with a carrier sequence ([`ON`] / [`OFF`]) that [`Overlined`]
//! turns back into the template change. Carriers travel in the byte stream, so they keep their
//! order even inside synchronized output, which vte buffers and replays.
//!
//! alacritty's reflow drops trailing cells it considers empty and its `is_empty` ignores
//! unknown flags, so an overlined blank with the default foreground would vanish on resize.
//! Those cells carry [`MARKED_FG`] instead of the default foreground: no SGR produces it and the
//! renderer reads it as the default.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, Color, CursorShape, CursorStyle, Handler, Hyperlink,
    KeyboardModes, KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, NamedColor,
    PrivateMode, Rgb, ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
    cursor_icon::CursorIcon,
};

/// The overline bit (alacritty uses bits 0 to 14).
pub(crate) const OVERLINE: Flags = Flags::from_bits_retain(1 << 15);

/// The default foreground of an overlined cell (see the module docs).
pub(crate) const MARKED_FG: Color = Color::Named(NamedColor::DimForeground);

/// Carriers: the title stack push / pop (`CSI 22 t` / `CSI 23 t`) for overline, the size reports
/// (`CSI 14 t` / `CSI 18 t`) for not bold / not faint. xterm.js disables window operations and
/// the scanner drops every `CSI t` from the PTY, so only the screen sends these.
pub(crate) const ON: &[u8] = b"\x1b[22t";
pub(crate) const OFF: &[u8] = b"\x1b[23t";
pub(crate) const NOT_BOLD: &[u8] = b"\x1b[14t";
pub(crate) const NOT_FAINT: &[u8] = b"\x1b[18t";

/// xterm.js's `Params` limits: parameters, and sub-parameters across the whole sequence.
const MAX_PARAMS: usize = 32;
const MAX_SUB_PARAMS: usize = 32;

pub(crate) fn is_overlined(cell: &Cell) -> bool {
    cell.flags.contains(OVERLINE)
}

/// What an SGR sequence (its parameter bytes, between `CSI` and `m`) does to overline, read the
/// way xterm.js's `charAttributes` walks it: `Some(true)` sets it, `Some(false)` clears it (55 or
/// a reset), `None` leaves it. Colour arguments of 38 / 48 / 58 are skipped as `_extractColor`
/// consumes them, so `38;5;53` is a colour, not an overline.
pub(crate) fn sgr_effect(body: &[u8]) -> Option<bool> {
    let mut effect = None;
    walk_sgr(body, |code| match code {
        0 | 55 => effect = Some(false),
        53 => effect = Some(true),
        _ => {}
    });
    effect
}

/// Whether an SGR sequence ends with bold cleared by 221, and faint by 222: vte applies the rest
/// of the sequence, so only a 221 / 222 that no later 0, 1, 2 or 22 overrides needs a carrier.
pub(crate) fn sgr_cancels(body: &[u8]) -> (bool, bool) {
    let (mut not_bold, mut not_faint) = (false, false);
    walk_sgr(body, |code| match code {
        221 => not_bold = true,
        222 => not_faint = true,
        0 | 22 => (not_bold, not_faint) = (false, false),
        1 => not_bold = false,
        2 => not_faint = false,
        _ => {}
    });
    (not_bold, not_faint)
}

/// Calls `f` with each attribute code of an SGR sequence, in order, skipping colour arguments.
fn walk_sgr(body: &[u8], mut f: impl FnMut(i64)) {
    // A private marker after the first byte makes the parser ignore the sequence.
    if !body
        .iter()
        .all(|b| b.is_ascii_digit() || *b == b';' || *b == b':')
    {
        return;
    }
    // (value, has stored sub-parameters), with empty parameters as 0.
    let mut subs = 0;
    let params: Vec<(i64, bool)> = body
        .split(|&b| b == b';')
        .take(MAX_PARAMS)
        .map(|p| {
            let mut parts = p.split(|&b| b == b':');
            let value = parts.next().unwrap_or_default().iter().fold(0i64, |n, b| {
                (n * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX))
            });
            let stored = parts.count().min(MAX_SUB_PARAMS - subs);
            subs += stored;
            (value, stored > 0)
        })
        .collect();
    let mut i = 0;
    while i < params.len() {
        match params[i].0 {
            38 | 48 | 58 => i += color_advance(&params, i),
            code => f(code),
        }
        i += 1;
    }
}

/// How many parameters after `pos` `InputHandler._extractColor` consumes.
fn color_advance(params: &[(i64, bool)], pos: usize) -> usize {
    let mut accu = [0i64, 0, -1, 0, 0, 0];
    let mut cspace = 0;
    let mut advance = 0;
    loop {
        accu[advance + cspace] = params[pos + advance].0;
        if params[pos + advance].1 {
            break;
        }
        if (accu[1] == 5 && advance + cspace >= 2) || (accu[1] == 2 && advance + cspace >= 5) {
            break;
        }
        if accu[1] != 0 {
            cspace = 1;
        }
        advance += 1;
        if advance + pos >= params.len() || advance + cspace >= accu.len() {
            break;
        }
    }
    advance
}

/// The emulator as vte's handler, with the overline carriers applied to the cursor template.
pub(crate) struct Overlined<'a, L: EventListener>(pub(crate) &'a mut Term<L>);

impl<L: EventListener> Overlined<'_, L> {
    fn set(&mut self, on: bool) {
        self.0.grid_mut().cursor.template.flags.set(OVERLINE, on);
        self.mark();
    }

    /// Keeps the template's foreground marked exactly while overline is on.
    fn mark(&mut self) {
        let t = &mut self.0.grid_mut().cursor.template;
        let on = t.flags.contains(OVERLINE);
        if on && t.fg == Color::Named(NamedColor::Foreground) {
            t.fg = MARKED_FG;
        } else if !on && t.fg == MARKED_FG {
            t.fg = Color::Named(NamedColor::Foreground);
        }
    }
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(fn $name(&mut self, $($arg: $ty),*) {
            self.0.$name($($arg),*)
        })*
    };
}

impl<L: EventListener> Handler for Overlined<'_, L> {
    fn terminal_attribute(&mut self, attr: Attr) {
        // vte reads SGR 21 (its only `CancelBold`) as "not bold", xterm.js as a double underline.
        let attr = match attr {
            Attr::CancelBold => Attr::DoubleUnderline,
            a => a,
        };
        self.0.terminal_attribute(attr);
        self.mark();
    }

    fn push_title(&mut self) {
        self.set(true);
    }

    fn pop_title(&mut self) {
        self.set(false);
    }

    fn text_area_size_pixels(&mut self) {
        self.0.grid_mut().cursor.template.flags.remove(Flags::BOLD);
    }

    fn text_area_size_chars(&mut self) {
        self.0.grid_mut().cursor.template.flags.remove(Flags::DIM);
    }

    delegate! {
        set_title(t: Option<String>);
        set_cursor_style(s: Option<CursorStyle>);
        set_cursor_shape(s: CursorShape);
        input(c: char);
        goto(line: i32, col: usize);
        goto_line(line: i32);
        goto_col(col: usize);
        insert_blank(n: usize);
        move_up(n: usize);
        move_down(n: usize);
        identify_terminal(i: Option<char>);
        device_status(n: usize);
        move_forward(col: usize);
        move_backward(col: usize);
        move_down_and_cr(row: usize);
        move_up_and_cr(row: usize);
        put_tab(count: u16);
        backspace();
        carriage_return();
        linefeed();
        bell();
        substitute();
        newline();
        set_horizontal_tabstop();
        scroll_up(n: usize);
        scroll_down(n: usize);
        insert_blank_lines(n: usize);
        delete_lines(n: usize);
        erase_chars(n: usize);
        delete_chars(n: usize);
        move_backward_tabs(count: u16);
        move_forward_tabs(count: u16);
        save_cursor_position();
        restore_cursor_position();
        clear_line(mode: LineClearMode);
        clear_screen(mode: ClearMode);
        clear_tabs(mode: TabulationClearMode);
        set_tabs(interval: u16);
        reset_state();
        reverse_index();
        set_mode(mode: Mode);
        unset_mode(mode: Mode);
        report_mode(mode: Mode);
        set_private_mode(mode: PrivateMode);
        unset_private_mode(mode: PrivateMode);
        report_private_mode(mode: PrivateMode);
        set_scrolling_region(top: usize, bottom: Option<usize>);
        set_keypad_application_mode();
        unset_keypad_application_mode();
        set_active_charset(c: CharsetIndex);
        configure_charset(c: CharsetIndex, s: StandardCharset);
        set_color(i: usize, c: Rgb);
        dynamic_color_sequence(p: String, i: usize, t: &str);
        reset_color(i: usize);
        clipboard_store(c: u8, d: &[u8]);
        clipboard_load(c: u8, t: &str);
        decaln();
        set_hyperlink(h: Option<Hyperlink>);
        set_mouse_cursor_icon(i: CursorIcon);
        report_keyboard_mode();
        push_keyboard_mode(mode: KeyboardModes);
        pop_keyboard_modes(n: u16);
        set_keyboard_mode(mode: KeyboardModes, b: KeyboardModesApplyBehavior);
        set_modify_other_keys(mode: ModifyOtherKeys);
        report_modify_other_keys();
        set_scp(p: ScpCharPath, u: ScpUpdateMode);
    }
}

#[cfg(test)]
mod tests {
    use super::{sgr_cancels, sgr_effect};

    #[test]
    fn sgr_cancels_follow_the_last_code() {
        assert_eq!(sgr_cancels(b"221"), (true, false));
        assert_eq!(sgr_cancels(b"222"), (false, true));
        assert_eq!(sgr_cancels(b"1;2;221;222"), (true, true));
        assert_eq!(sgr_cancels(b"221;1"), (false, false), "a later 1 wins");
        assert_eq!(sgr_cancels(b"222;0"), (false, false));
        assert_eq!(sgr_cancels(b"38;5;221"), (false, false), "a colour argument");
        assert_eq!(sgr_cancels(b"221:3"), (true, false));
    }

    #[test]
    fn sgr_effect_follows_xterm_parameter_order() {
        assert_eq!(sgr_effect(b"53"), Some(true));
        assert_eq!(sgr_effect(b"55"), Some(false));
        assert_eq!(sgr_effect(b"1;31"), None);
        assert_eq!(sgr_effect(b""), Some(false), "CSI m resets");
        assert_eq!(sgr_effect(b"53;0"), Some(false));
        assert_eq!(sgr_effect(b"0;53"), Some(true));
        assert_eq!(sgr_effect(b";53"), Some(true), "an empty parameter is 0");
        assert_eq!(sgr_effect(b"53;55;53"), Some(true));
        assert_eq!(
            sgr_effect(b"53:1"),
            Some(true),
            "sub-parameters are ignored"
        );
    }

    #[test]
    fn colour_arguments_are_not_attributes() {
        assert_eq!(sgr_effect(b"38;5;53"), None);
        assert_eq!(sgr_effect(b"48;5;55"), None);
        assert_eq!(sgr_effect(b"38;2;53;55;0"), None);
        assert_eq!(sgr_effect(b"38;2;1;2;3;53"), Some(true));
        assert_eq!(sgr_effect(b"58:5:53;53"), Some(true));
        assert_eq!(sgr_effect(b"38:2::1:2:3;55"), Some(false));
        // An unknown colour mode swallows up to four more parameters.
        assert_eq!(sgr_effect(b"38;9;53;53;53;53"), None);
        assert_eq!(sgr_effect(b"38;9;1;1;1;1;53"), Some(true));
        assert_eq!(sgr_effect(b"38;5"), None);
    }

    #[test]
    fn params_past_xterm_limit_are_dropped() {
        let mut body = "1;".repeat(32);
        body.push_str("53");
        assert_eq!(sgr_effect(body.as_bytes()), None);
    }
}
