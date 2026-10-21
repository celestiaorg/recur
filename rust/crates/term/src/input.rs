//! Input encoding ported from xterm.js: keyboard (`Keyboard.ts` plus the keydown/keypress split
//! in `CoreBrowserTerminal`), mouse reports (`MouseStateService`, `MouseService`), paste
//! (`Clipboard.ts`) and pointer-to-cell rounding (`Mouse.ts`).

/// A key press as the UI toolkit reports it: `key` is the unmodified key name (`"a"`, `"enter"`,
/// `"left"`, `"f5"`, `"!"`), `key_char` the text it would type, if any.
#[derive(Debug, Clone, Default)]
pub struct KeyPress<'a> {
    pub key: &'a str,
    pub key_char: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// What the terminal does with a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Write these bytes to the PTY (user input: scroll to bottom, clear selection).
    Send(String),
    /// Shift+PageUp / Shift+PageDown scroll the viewport by a page.
    PageUp,
    PageDown,
    /// Let the platform text input deliver the character (plain typing, Option compose, IME).
    Text,
    /// Not the terminal's key; let it propagate to the app.
    Ignore,
}

/// The DOM view of a key press xterm.js evaluates.
struct Dom {
    key_code: u32,
    key: String,
    code: String,
    shift: bool,
}

const SHIFTED: &[(char, char)] = &[
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

fn punct_code(c: char) -> Option<(u32, &'static str)> {
    Some(match c {
        ';' => (186, "Semicolon"),
        '=' => (187, "Equal"),
        ',' => (188, "Comma"),
        '-' => (189, "Minus"),
        '.' => (190, "Period"),
        '/' => (191, "Slash"),
        '`' => (192, "Backquote"),
        '[' => (219, "BracketLeft"),
        '\\' => (220, "Backslash"),
        ']' => (221, "BracketRight"),
        '\'' => (222, "Quote"),
        _ => return None,
    })
}

fn to_dom(k: &KeyPress) -> Dom {
    let named = match k.key {
        "backspace" => Some((8, "Backspace", "Backspace")),
        "tab" => Some((9, "Tab", "Tab")),
        "enter" => Some((13, "Enter", "Enter")),
        "escape" => Some((27, "Escape", "Escape")),
        "space" => Some((32, " ", "Space")),
        "pageup" => Some((33, "PageUp", "PageUp")),
        "pagedown" => Some((34, "PageDown", "PageDown")),
        "end" => Some((35, "End", "End")),
        "home" => Some((36, "Home", "Home")),
        "left" => Some((37, "ArrowLeft", "ArrowLeft")),
        "up" => Some((38, "ArrowUp", "ArrowUp")),
        "right" => Some((39, "ArrowRight", "ArrowRight")),
        "down" => Some((40, "ArrowDown", "ArrowDown")),
        "insert" => Some((45, "Insert", "Insert")),
        "delete" => Some((46, "Delete", "Delete")),
        _ => None,
    };
    if let Some((code, key, dom_code)) = named {
        return Dom {
            key_code: code,
            key: key.into(),
            code: dom_code.into(),
            shift: k.shift,
        };
    }
    if let Some(n) = k
        .key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=24).contains(n))
    {
        return Dom {
            key_code: 111 + n,
            key: format!("F{n}"),
            code: format!("F{n}"),
            shift: k.shift,
        };
    }
    let mut chars = k.key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Dom {
            key_code: 0,
            key: k.key_char.unwrap_or(k.key).into(),
            code: String::new(),
            shift: k.shift,
        };
    };
    // GPUI reports shifted punctuation as the shifted character with shift cleared; DOM reports
    // the physical key with shift held.
    let (base, shift) = match SHIFTED.iter().find(|(s, _)| *s == c) {
        Some(&(_, b)) => (b, true),
        None => (c, k.shift),
    };
    let key = match k.key_char {
        Some(t) if !t.is_empty() => t.to_string(),
        Some(_) => "Dead".to_string(),
        None => c.to_string(),
    };
    let (key_code, code) = if base.is_ascii_alphabetic() {
        (
            base.to_ascii_uppercase() as u32,
            format!("Key{}", base.to_ascii_uppercase()),
        )
    } else if base.is_ascii_digit() {
        (base as u32, format!("Digit{base}"))
    } else if let Some((kc, name)) = punct_code(base) {
        (kc, name.to_string())
    } else {
        // Non-Latin layouts: no keyCode xterm.js recognises, only the character.
        (u32::MAX, String::new())
    };
    Dom {
        key_code,
        key,
        code,
        shift,
    }
}

const ESC: &str = "\x1b";

/// xterm.js `evaluateKeyboardEvent` plus the keydown decisions in `CoreBrowserTerminal._keyDown`
/// for a pane configured like Recur's (`macOptionIsMeta: false`, the dock's custom key handler
/// that leaves ⌘ chords and ⌃\`/⌃⇧\` to the app).
pub fn evaluate_key(
    k: &KeyPress,
    app_cursor: bool,
    is_mac: bool,
    mac_option_is_meta: bool,
) -> KeyAction {
    // The pane's attachCustomKeyEventHandler.
    if k.meta {
        return KeyAction::Ignore;
    }
    let ev = to_dom(k);
    if k.ctrl && !k.alt && ev.code == "Backquote" {
        return KeyAction::Ignore;
    }
    let (shift, alt, ctrl, meta) = (ev.shift, k.alt, k.ctrl, k.meta);
    let modifiers =
        u32::from(shift) | u32::from(alt) << 1 | u32::from(ctrl) << 2 | u32::from(meta) << 3;
    let csi_mod = |base: &str, fin: char| format!("{ESC}[{base};{}{fin}", modifiers + 1);
    let cursor = |fin: char| {
        if modifiers != 0 {
            csi_mod("1", fin)
        } else if app_cursor {
            format!("{ESC}O{fin}")
        } else {
            format!("{ESC}[{fin}")
        }
    };
    let key: Option<String> = match ev.key_code {
        8 => {
            let k = if ctrl { "\x08" } else { "\x7f" };
            Some(if alt {
                format!("{ESC}{k}")
            } else {
                k.to_string()
            })
        }
        9 => Some(if shift {
            format!("{ESC}[Z")
        } else {
            "\t".into()
        }),
        13 => Some(if alt { format!("{ESC}\r") } else { "\r".into() }),
        27 => Some(if alt {
            format!("{ESC}{ESC}")
        } else {
            ESC.into()
        }),
        37 => (!meta).then(|| cursor('D')),
        39 => (!meta).then(|| cursor('C')),
        38 => (!meta).then(|| cursor('A')),
        40 => (!meta).then(|| cursor('B')),
        45 => (!shift && !ctrl).then(|| format!("{ESC}[2~")),
        46 => Some(if modifiers != 0 {
            csi_mod("3", '~')
        } else {
            format!("{ESC}[3~")
        }),
        36 => Some(cursor('H')),
        35 => Some(cursor('F')),
        33 => {
            if shift {
                return KeyAction::PageUp;
            }
            Some(if ctrl {
                csi_mod("5", '~')
            } else {
                format!("{ESC}[5~")
            })
        }
        34 => {
            if shift {
                return KeyAction::PageDown;
            }
            Some(if ctrl {
                csi_mod("6", '~')
            } else {
                format!("{ESC}[6~")
            })
        }
        112..=115 => {
            let fin = ['P', 'Q', 'R', 'S'][(ev.key_code - 112) as usize];
            Some(if modifiers != 0 {
                csi_mod("1", fin)
            } else {
                format!("{ESC}O{fin}")
            })
        }
        116..=123 => {
            let n = [15, 17, 18, 19, 20, 21, 23, 24][(ev.key_code - 116) as usize];
            Some(if modifiers != 0 {
                csi_mod(&n.to_string(), '~')
            } else {
                format!("{ESC}[{n}~")
            })
        }
        code => default_key(
            &ev,
            code,
            shift,
            alt,
            ctrl,
            meta,
            is_mac,
            mac_option_is_meta,
        ),
    };

    // _isThirdLevelShift: Option composes characters on macOS; let the text input deliver them.
    let third_level = is_mac && !mac_option_is_meta && alt && !ctrl && !meta;
    if third_level && (ev.key_code == 0 || ev.key_code > 47) {
        return KeyAction::Text;
    }
    let Some(key) = key else {
        // Unhandled keydowns fall through to keypress, which sends the typed character only
        // when no modifier (other than shift) is held.
        return if ctrl || alt || meta {
            KeyAction::Ignore
        } else {
            KeyAction::Text
        };
    };
    // Plain typing: xterm sends `ev.key` itself; the platform text input delivers the same
    // character and also covers caps lock and input methods.
    if !ctrl
        && !alt
        && !meta
        && k.key_char == Some(key.as_str())
        && key.chars().count() == 1
        && ev.key_code >= 48
    {
        return KeyAction::Text;
    }
    KeyAction::Send(key)
}

#[allow(clippy::too_many_arguments)]
fn default_key(
    ev: &Dom,
    code: u32,
    shift: bool,
    alt: bool,
    ctrl: bool,
    meta: bool,
    is_mac: bool,
    option_meta: bool,
) -> Option<String> {
    if ctrl && !shift && !alt && !meta {
        return match code {
            65..=90 => Some(((code - 64) as u8 as char).to_string()),
            32 => Some("\0".into()),
            51..=55 => Some(((code - 51 + 27) as u8 as char).to_string()),
            56 => Some("\x7f".into()),
            _ if ev.key == "/" => Some("\x1f".into()),
            219 => Some(ESC.into()),
            220 => Some("\x1c".into()),
            221 => Some("\x1d".into()),
            _ => None,
        };
    }
    if (!is_mac || option_meta) && alt && !meta {
        const MAP: &[(u32, char, char)] = &[
            (48, '0', ')'),
            (49, '1', '!'),
            (50, '2', '@'),
            (51, '3', '#'),
            (52, '4', '$'),
            (53, '5', '%'),
            (54, '6', '^'),
            (55, '7', '&'),
            (56, '8', '*'),
            (57, '9', '('),
            (186, ';', ':'),
            (187, '=', '+'),
            (188, ',', '<'),
            (189, '-', '_'),
            (190, '.', '>'),
            (191, '/', '?'),
            (192, '`', '~'),
            (219, '[', '{'),
            (220, '\\', '|'),
            (221, ']', '}'),
            (222, '\'', '"'),
        ];
        if let Some(&(_, plain, shifted)) = MAP.iter().find(|m| m.0 == code) {
            return Some(format!("{ESC}{}", if shift { shifted } else { plain }));
        }
        if (65..=90).contains(&code) {
            let kc = if ctrl { code - 64 } else { code + 32 };
            let mut s = (kc as u8 as char).to_string();
            if shift {
                s = s.to_uppercase();
            }
            return Some(format!("{ESC}{s}"));
        }
        if code == 32 {
            return Some(format!("{ESC}{}", if ctrl { "\0" } else { " " }));
        }
        if ev.key == "Dead" && ev.code.starts_with("Key") {
            let mut s = ev.code[3..4].to_string();
            if !shift {
                s = s.to_lowercase();
            }
            return Some(format!("{ESC}{s}"));
        }
        return None;
    }
    if is_mac && !alt && !ctrl && !shift && meta {
        return None;
    }
    if !ctrl && !alt && !meta && code >= 48 && ev.key.chars().count() == 1 {
        return Some(ev.key.clone());
    }
    if ctrl && shift {
        return match ev.code.as_str() {
            "Minus" => Some("\x1f".into()),
            "Digit2" => Some("\0".into()),
            "Digit6" => Some("\x1e".into()),
            _ => None,
        };
    }
    None
}

/// xterm `prepareTextForTerminal` + `bracketTextForPaste`.
pub fn paste_text(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text;
    }
    // Escape characters could end bracketed paste early; xterm shows them as U+241B.
    format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "\u{241b}"))
}

/// xterm `getCoords(..., isSelection)`: 1-based cell for a pointer at `(x, y)` relative to the
/// grid's top-left, clamped to the grid (selection gets one extra column).
pub fn cell_coords(
    x: f64,
    y: f64,
    cell_w: f64,
    cell_h: f64,
    cols: usize,
    rows: usize,
    selection: bool,
) -> (i64, i64) {
    let cx = ((x + if selection { cell_w / 2.0 } else { 0.0 }) / cell_w).ceil() as i64;
    let cy = (y / cell_h).ceil() as i64;
    (
        cx.clamp(1, cols as i64 + i64::from(selection)),
        cy.clamp(1, rows as i64),
    )
}

/// Mouse buttons as xterm's `CoreMouseButton`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    None = 3,
    Wheel = 4,
}

/// xterm's `CoreMouseAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    /// Release, or wheel up.
    Up = 0,
    /// Press, or wheel down.
    Down = 1,
    Move = 32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseProtocol {
    #[default]
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    #[default]
    Default,
    Sgr,
    SgrPixels,
}

impl MouseProtocol {
    pub fn wants_wheel(self) -> bool {
        matches!(self, Self::Vt200 | Self::Drag | Self::Any)
    }

    pub fn wants_up(self) -> bool {
        self.wants_wheel()
    }

    pub fn wants_drag(self) -> bool {
        matches!(self, Self::Drag | Self::Any)
    }

    pub fn wants_move(self) -> bool {
        self == Self::Any
    }
}

/// One mouse report request. `col`/`row` are 0-based cells, `x`/`y` pixels in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseReport {
    pub col: i64,
    pub row: i64,
    pub x: i64,
    pub y: i64,
    pub button: MouseButton,
    pub action: MouseAction,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// What a report encodes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoded {
    /// `onData`: SGR reports, sent as user input.
    Data(String),
    /// `onBinary`: legacy X10 encoding. Recur's pane never subscribed to `onBinary`, so these
    /// never reached the PTY.
    Binary(String),
}

fn event_code(e: &MouseReport, sgr: bool) -> u32 {
    let mut code = (u32::from(e.ctrl) * 16) | (u32::from(e.shift) * 4) | (u32::from(e.alt) * 8);
    if e.button == MouseButton::Wheel {
        code |= 64 | e.action as u32;
    } else {
        code |= e.button as u32 & 3;
        if e.action == MouseAction::Move {
            code |= 32;
        } else if e.action == MouseAction::Up && !sgr {
            code |= MouseButton::None as u32;
        }
    }
    code
}

/// Debounces identical move reports and applies protocol restrictions and encoding, like
/// `MouseService._triggerMouseEvent`.
#[derive(Debug, Default)]
pub struct MouseReporter {
    last: Option<MouseReport>,
    wheel_partial: f64,
}

impl MouseReporter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn report(
        &mut self,
        mut e: MouseReport,
        cols: usize,
        rows: usize,
        protocol: MouseProtocol,
        encoding: MouseEncoding,
    ) -> Option<Encoded> {
        if e.col < 0 || e.col >= cols as i64 || e.row < 0 || e.row >= rows as i64 {
            return None;
        }
        if e.button == MouseButton::Wheel && e.action == MouseAction::Move {
            return None;
        }
        if e.button == MouseButton::None && e.action != MouseAction::Move {
            return None;
        }
        e.col += 1;
        e.row += 1;
        if e.action == MouseAction::Move
            && let Some(last) = &self.last
        {
            let same_pos = if encoding == MouseEncoding::SgrPixels {
                (last.x, last.y) == (e.x, e.y)
            } else {
                (last.col, last.row) == (e.col, e.row)
            };
            if same_pos
                && (last.button, last.action, last.ctrl, last.alt, last.shift)
                    == (e.button, e.action, e.ctrl, e.alt, e.shift)
            {
                return None;
            }
        }
        match protocol {
            MouseProtocol::None => return None,
            MouseProtocol::X10 => {
                if e.button == MouseButton::Wheel || e.action != MouseAction::Down {
                    return None;
                }
                e.ctrl = false;
                e.alt = false;
                e.shift = false;
            }
            MouseProtocol::Vt200 if e.action == MouseAction::Move => return None,
            MouseProtocol::Drag
                if e.action == MouseAction::Move && e.button == MouseButton::None =>
            {
                return None;
            }
            _ => {}
        }
        self.last = Some(e);
        let fin = if e.action == MouseAction::Up && e.button != MouseButton::Wheel {
            'm'
        } else {
            'M'
        };
        Some(match encoding {
            MouseEncoding::Default => {
                let p = [
                    event_code(&e, false) + 32,
                    e.col as u32 + 32,
                    e.row as u32 + 32,
                ];
                if p.iter().any(|&v| v > 255) {
                    return None;
                }
                let s: String = p.iter().filter_map(|&v| char::from_u32(v)).collect();
                Encoded::Binary(format!("\x1b[M{s}"))
            }
            MouseEncoding::Sgr => Encoded::Data(format!(
                "\x1b[<{};{};{}{fin}",
                event_code(&e, true),
                e.col,
                e.row
            )),
            MouseEncoding::SgrPixels => Encoded::Data(format!(
                "\x1b[<{};{};{}{fin}",
                event_code(&e, true),
                e.x,
                e.y
            )),
        })
    }

    /// `_consumeWheelEvent`: whole lines for a DOM-style wheel delta (`delta_y` positive scrolls
    /// down). `pixels` is DOM_DELTA_PIXEL mode; otherwise lines.
    pub fn consume_wheel(
        &mut self,
        delta_y: f64,
        pixels: bool,
        cell_height: f64,
        rows: usize,
        fast: bool,
        shift: bool,
    ) -> i64 {
        if delta_y == 0.0 || shift || cell_height <= 0.0 {
            return 0;
        }
        let mut amount = delta_y * if fast { FAST_SCROLL_SENSITIVITY } else { 1.0 };
        if pixels {
            amount /= cell_height;
            if delta_y.abs() < 50.0 {
                amount *= 0.3;
            }
            self.wheel_partial += amount;
            let whole = self.wheel_partial.abs().floor() * self.wheel_partial.signum();
            self.wheel_partial %= 1.0;
            whole as i64
        } else {
            (amount * rows as f64) as i64
        }
    }
}

/// xterm.js `fastScrollSensitivity`.
pub const FAST_SCROLL_SENSITIVITY: f64 = 5.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str, key_char: Option<&str>) -> KeyPress<'static> {
        KeyPress {
            key: Box::leak(key.to_string().into_boxed_str()),
            key_char: key_char.map(|s| &*Box::leak(s.to_string().into_boxed_str())),
            ..Default::default()
        }
    }

    fn eval(k: KeyPress) -> KeyAction {
        evaluate_key(&k, false, true, false)
    }

    #[test]
    fn plain_typing_goes_through_text_input() {
        assert_eq!(eval(key("a", Some("a"))), KeyAction::Text);
        assert_eq!(
            eval(KeyPress {
                shift: true,
                ..key("a", Some("A"))
            }),
            KeyAction::Text
        );
        assert_eq!(eval(key("space", Some(" "))), KeyAction::Text);
        assert_eq!(eval(key("1", Some("1"))), KeyAction::Text);
    }

    #[test]
    fn special_keys() {
        assert_eq!(eval(key("enter", Some("\n"))), KeyAction::Send("\r".into()));
        assert_eq!(eval(key("backspace", None)), KeyAction::Send("\x7f".into()));
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("backspace", None)
            }),
            KeyAction::Send("\x08".into())
        );
        assert_eq!(eval(key("tab", Some("\t"))), KeyAction::Send("\t".into()));
        assert_eq!(
            eval(KeyPress {
                shift: true,
                ..key("tab", Some("\t"))
            }),
            KeyAction::Send("\x1b[Z".into())
        );
        assert_eq!(eval(key("escape", None)), KeyAction::Send("\x1b".into()));
        assert_eq!(eval(key("up", None)), KeyAction::Send("\x1b[A".into()));
        assert_eq!(
            evaluate_key(&key("up", None), true, true, false),
            KeyAction::Send("\x1bOA".into())
        );
        assert_eq!(
            eval(KeyPress {
                alt: true,
                ..key("left", None)
            }),
            KeyAction::Send("\x1b[1;3D".into())
        );
        assert_eq!(eval(key("delete", None)), KeyAction::Send("\x1b[3~".into()));
        assert_eq!(eval(key("f1", None)), KeyAction::Send("\x1bOP".into()));
        assert_eq!(
            eval(KeyPress {
                shift: true,
                ..key("f5", None)
            }),
            KeyAction::Send("\x1b[15;2~".into())
        );
        assert_eq!(eval(key("f12", None)), KeyAction::Send("\x1b[24~".into()));
        assert_eq!(
            eval(KeyPress {
                shift: true,
                ..key("pageup", None)
            }),
            KeyAction::PageUp
        );
        assert_eq!(
            eval(key("pagedown", None)),
            KeyAction::Send("\x1b[6~".into())
        );
        assert_eq!(
            eval(KeyPress {
                alt: true,
                ..key("enter", Some("\n"))
            }),
            KeyAction::Send("\x1b\r".into())
        );
    }

    #[test]
    fn control_chords() {
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("c", None)
            }),
            KeyAction::Send("\x03".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("space", None)
            }),
            KeyAction::Send("\0".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("[", None)
            }),
            KeyAction::Send("\x1b".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("/", None)
            }),
            KeyAction::Send("\x1f".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("@", None)
            }),
            KeyAction::Send("\0".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("^", None)
            }),
            KeyAction::Send("\x1e".into())
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                shift: true,
                ..key("a", None)
            }),
            KeyAction::Ignore
        );
    }

    #[test]
    fn app_owned_chords_propagate() {
        assert_eq!(
            eval(KeyPress {
                meta: true,
                ..key("t", None)
            }),
            KeyAction::Ignore
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("`", None)
            }),
            KeyAction::Ignore
        );
        assert_eq!(
            eval(KeyPress {
                ctrl: true,
                ..key("~", None)
            }),
            KeyAction::Ignore
        );
    }

    #[test]
    fn option_composes_on_mac_and_is_meta_elsewhere() {
        assert_eq!(
            eval(KeyPress {
                alt: true,
                ..key("a", Some("å"))
            }),
            KeyAction::Text
        );
        let linux = |k: KeyPress| evaluate_key(&k, false, false, false);
        assert_eq!(
            linux(KeyPress {
                alt: true,
                ..key("a", Some("a"))
            }),
            KeyAction::Send("\x1ba".into())
        );
        assert_eq!(
            linux(KeyPress {
                alt: true,
                shift: true,
                ..key("a", Some("A"))
            }),
            KeyAction::Send("\x1bA".into())
        );
        assert_eq!(
            linux(KeyPress {
                alt: true,
                ..key(".", Some("."))
            }),
            KeyAction::Send("\x1b.".into())
        );
        assert_eq!(
            linux(KeyPress {
                alt: true,
                ..key("?", Some("?"))
            }),
            KeyAction::Send("\x1b?".into())
        );
        assert_eq!(
            evaluate_key(
                &KeyPress {
                    alt: true,
                    ..key("b", Some("∫"))
                },
                false,
                true,
                true
            ),
            KeyAction::Send("\x1bb".into())
        );
    }

    #[test]
    fn paste_converts_newlines_and_brackets() {
        assert_eq!(paste_text("a\nb\r\nc", false), "a\rb\rc");
        assert_eq!(paste_text("x\x1by", true), "\x1b[200~x\u{241b}y\x1b[201~");
    }

    #[test]
    fn cell_coords_round_like_xterm() {
        assert_eq!(cell_coords(0.0, 0.0, 8.0, 16.0, 80, 24, false), (1, 1));
        assert_eq!(cell_coords(9.0, 17.0, 8.0, 16.0, 80, 24, false), (2, 2));
        assert_eq!(cell_coords(3.0, 1.0, 8.0, 16.0, 80, 24, true), (1, 1));
        assert_eq!(cell_coords(5.0, 1.0, 8.0, 16.0, 80, 24, true), (2, 1));
        assert_eq!(
            cell_coords(10_000.0, 10_000.0, 8.0, 16.0, 80, 24, true),
            (81, 24)
        );
    }

    fn rep(button: MouseButton, action: MouseAction) -> MouseReport {
        MouseReport {
            col: 4,
            row: 2,
            x: 37,
            y: 40,
            button,
            action,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    #[test]
    fn sgr_reports() {
        let mut r = MouseReporter::default();
        let (p, sgr) = (MouseProtocol::Vt200, MouseEncoding::Sgr);
        assert_eq!(
            r.report(rep(MouseButton::Left, MouseAction::Down), 80, 24, p, sgr),
            Some(Encoded::Data("\x1b[<0;5;3M".into()))
        );
        assert_eq!(
            r.report(rep(MouseButton::Left, MouseAction::Up), 80, 24, p, sgr),
            Some(Encoded::Data("\x1b[<0;5;3m".into()))
        );
        assert_eq!(
            r.report(rep(MouseButton::Wheel, MouseAction::Up), 80, 24, p, sgr),
            Some(Encoded::Data("\x1b[<64;5;3M".into()))
        );
        assert_eq!(
            r.report(rep(MouseButton::Wheel, MouseAction::Down), 80, 24, p, sgr),
            Some(Encoded::Data("\x1b[<65;5;3M".into()))
        );
        assert_eq!(
            r.report(rep(MouseButton::Left, MouseAction::Move), 80, 24, p, sgr),
            None,
            "vt200 has no motion"
        );
        assert_eq!(
            r.report(
                rep(MouseButton::Left, MouseAction::Move),
                80,
                24,
                MouseProtocol::Drag,
                MouseEncoding::SgrPixels
            ),
            Some(Encoded::Data("\x1b[<32;37;40M".into()))
        );
        assert_eq!(
            r.report(
                rep(MouseButton::Left, MouseAction::Move),
                80,
                24,
                MouseProtocol::Drag,
                MouseEncoding::SgrPixels
            ),
            None,
            "debounced"
        );
        assert_eq!(
            r.report(
                rep(MouseButton::None, MouseAction::Move),
                80,
                24,
                MouseProtocol::Drag,
                sgr
            ),
            None
        );
    }

    #[test]
    fn default_encoding_is_binary_and_x10_strips_modifiers() {
        let mut r = MouseReporter::default();
        let e = MouseReport {
            ctrl: true,
            ..rep(MouseButton::Left, MouseAction::Down)
        };
        assert_eq!(
            r.report(e, 80, 24, MouseProtocol::X10, MouseEncoding::Default),
            Some(Encoded::Binary("\x1b[M %#".into()))
        );
        assert_eq!(
            r.report(
                rep(MouseButton::Left, MouseAction::Up),
                80,
                24,
                MouseProtocol::X10,
                MouseEncoding::Default
            ),
            None
        );
    }

    #[test]
    fn wheel_consumption_accumulates_partial_lines() {
        let mut r = MouseReporter::default();
        // Trackpad-size deltas are damped and accumulate.
        assert_eq!(r.consume_wheel(10.0, true, 16.0, 24, false, false), 0);
        assert_eq!(r.consume_wheel(40.0, true, 16.0, 24, false, false), 0);
        assert_eq!(r.consume_wheel(40.0, true, 16.0, 24, false, false), 1);
        assert_eq!(r.consume_wheel(-160.0, true, 16.0, 24, false, false), -9);
        assert_eq!(
            r.consume_wheel(3.0, false, 16.0, 24, false, true),
            0,
            "shift wheel is horizontal"
        );
    }
}
