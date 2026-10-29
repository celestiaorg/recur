//! Key bindings with CodeMirror's resolution rules (`keymap`,
//! `buildKeymap`, `runHandlers`, w3c-keyname's `keyName`): `Mod` is Cmd on
//! macOS and Ctrl elsewhere, bindings are tried in precedence order until
//! one handles the key, character keys fall back to their unshifted and
//! shifted names, and `scope` limits a binding to the editor or a panel.
//!
//! The command type is the caller's; bindings are compared by value so a
//! command reached through two names in one key press only runs once.

use std::collections::HashMap;

use gpui_kit::Keystroke;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Linux,
    Win,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_os = "windows") {
            Platform::Win
        } else {
            Platform::Linux
        }
    }
}

/// A `KeyBinding`: `key` with optional per-platform overrides, the
/// command, its Shift variant, and the scopes it applies in (default
/// `"editor"`).
#[derive(Clone, Debug)]
pub struct KeyBinding<C> {
    pub key: Option<&'static str>,
    pub mac: Option<&'static str>,
    pub linux: Option<&'static str>,
    pub win: Option<&'static str>,
    pub run: Option<C>,
    pub shift: Option<C>,
    pub scope: Option<&'static str>,
    pub prevent_default: bool,
}

impl<C> KeyBinding<C> {
    pub fn new(key: &'static str, run: C) -> KeyBinding<C> {
        KeyBinding {
            key: Some(key),
            mac: None,
            linux: None,
            win: None,
            run: Some(run),
            shift: None,
            scope: None,
            prevent_default: false,
        }
    }

    pub fn mac_only(mac: &'static str, run: C) -> KeyBinding<C> {
        KeyBinding {
            key: None,
            mac: Some(mac),
            ..KeyBinding::new("", run)
        }
    }

    pub fn mac(mut self, key: &'static str) -> Self {
        self.mac = Some(key);
        self
    }

    pub fn linux(mut self, key: &'static str) -> Self {
        self.linux = Some(key);
        self
    }

    pub fn shift(mut self, run: C) -> Self {
        self.shift = Some(run);
        self
    }

    pub fn scope(mut self, scope: &'static str) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn prevent_default(mut self) -> Self {
        self.prevent_default = true;
        self
    }

    fn name_for(&self, platform: Platform) -> Option<&'static str> {
        let specific = match platform {
            Platform::Mac => self.mac,
            Platform::Linux => self.linux,
            Platform::Win => self.win,
        };
        specific.or(self.key).filter(|k| !k.is_empty())
    }
}

/// `normalizeKeyName`: `Shift-Meta-Ctrl-Alt-<key>`, `Mod` resolved.
pub fn normalize_key_name(name: &str, platform: Platform) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = name;
    while let Some(i) = rest.find('-') {
        if i + 1 == rest.len() {
            break;
        }
        parts.push(&rest[..i]);
        rest = &rest[i + 1..];
    }
    let mut result = if rest == "Space" {
        " ".to_string()
    } else {
        rest.to_string()
    };
    let (mut alt, mut ctrl, mut shift, mut meta) = (false, false, false, false);
    for m in parts {
        match m.to_lowercase().as_str() {
            "cmd" | "meta" | "m" => meta = true,
            "a" | "alt" => alt = true,
            "c" | "ctrl" | "control" => ctrl = true,
            "s" | "shift" => shift = true,
            "mod" => {
                if platform == Platform::Mac {
                    meta = true;
                } else {
                    ctrl = true;
                }
            }
            _ => {}
        }
    }
    if alt {
        result = format!("Alt-{result}");
    }
    if ctrl {
        result = format!("Ctrl-{result}");
    }
    if meta {
        result = format!("Meta-{result}");
    }
    if shift {
        result = format!("Shift-{result}");
    }
    result
}

struct Bound<C> {
    run: Vec<C>,
    prevent_default: bool,
}

/// A built keymap: per scope, normalized key name to commands in
/// precedence order.
pub struct Keymap<C> {
    scopes: HashMap<&'static str, HashMap<String, Bound<C>>>,
}

impl<C: Clone + PartialEq> Keymap<C> {
    /// `buildKeymap`: bindings earlier in the list take precedence.
    pub fn build(bindings: &[KeyBinding<C>], platform: Platform) -> Keymap<C> {
        let mut scopes: HashMap<&'static str, HashMap<String, Bound<C>>> = HashMap::new();
        for b in bindings {
            let Some(name) = b.name_for(platform) else {
                continue;
            };
            let names: Vec<&'static str> =
                b.scope.map_or(vec!["editor"], |s| s.split(' ').collect());
            for scope in names {
                let map = scopes.entry(scope).or_default();
                let mut add = |key: String, cmd: &Option<C>| {
                    let bound = map.entry(key).or_insert_with(|| Bound {
                        run: Vec::new(),
                        prevent_default: false,
                    });
                    if let Some(c) = cmd {
                        bound.run.push(c.clone());
                    }
                    if b.prevent_default {
                        bound.prevent_default = true;
                    }
                };
                add(normalize_key_name(name, platform), &b.run);
                if b.shift.is_some() {
                    add(
                        normalize_key_name(&format!("Shift-{name}"), platform),
                        &b.shift,
                    );
                }
            }
        }
        Keymap { scopes }
    }

    /// `runHandlers`: try the bindings for `key` in `scope`, calling `run`
    /// until one returns true. Returns whether the key was handled
    /// (including bindings that only prevent the default).
    pub fn run(
        &self,
        key: &KeyEvent,
        scope: &str,
        platform: Platform,
        mut run: impl FnMut(&C) -> bool,
    ) -> bool {
        let Some(map) = self.scopes.get(scope) else {
            return false;
        };
        let mut ran: Vec<C> = Vec::new();
        let mut prevented = false;
        let mut run_for = |name: &str, ran: &mut Vec<C>, prevented: &mut bool| -> bool {
            let Some(bound) = map.get(name) else {
                return false;
            };
            for cmd in &bound.run {
                if ran.contains(cmd) {
                    continue;
                }
                ran.push(cmd.clone());
                if run(cmd) {
                    return true;
                }
            }
            if bound.prevent_default {
                *prevented = true;
            }
            false
        };
        let name = &key.name;
        let is_char = name.chars().count() == 1 && name != " ";
        let mut handled = run_for(&key.modifiers(name, !is_char), &mut ran, &mut prevented);
        if !handled {
            let modified = key.alt || key.meta || key.ctrl;
            let windows_alt_gr = platform == Platform::Win && key.ctrl && key.alt;
            let mac_typed = platform == Platform::Mac && key.alt && !(key.ctrl || key.meta);
            if is_char
                && modified
                && !windows_alt_gr
                && !mac_typed
                && key.base.as_deref().is_some_and(|b| b != name)
            {
                let base = key.base.clone().unwrap_or_default();
                if run_for(&key.modifiers(&base, true), &mut ran, &mut prevented) {
                    handled = true;
                } else if key.shift
                    && let Some(shifted) = key.shifted.as_deref()
                    && shifted != name
                    && shifted != base
                    && run_for(&key.modifiers(shifted, false), &mut ran, &mut prevented)
                {
                    handled = true;
                }
            } else if is_char
                && key.shift
                && run_for(&key.modifiers(name, true), &mut ran, &mut prevented)
            {
                handled = true;
            }
        }
        handled || prevented
    }
}

/// A key press in DOM terms: `event.key` (via `keyName`), the
/// layout-independent base and shifted names (`base[keyCode]` /
/// `shift[keyCode]`), and the modifier flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub name: String,
    pub base: Option<String>,
    pub shifted: Option<String>,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
}

/// w3c-keyname's US-layout `shift` table for non-letters.
const SHIFT_PAIRS: &[(char, char)] = &[
    ('0', ')'),
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    (';', ':'),
    ('=', '+'),
    ('-', '_'),
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
    ('`', '~'),
    ('[', '{'),
    ('\\', '|'),
    (']', '}'),
    ('\'', '"'),
];

impl KeyEvent {
    /// `modifiers(name, event, shift)`.
    fn modifiers(&self, name: &str, with_shift: bool) -> String {
        let mut out = name.to_string();
        if self.alt {
            out = format!("Alt-{out}");
        }
        if self.ctrl {
            out = format!("Ctrl-{out}");
        }
        if self.meta {
            out = format!("Meta-{out}");
        }
        if with_shift && self.shift {
            out = format!("Shift-{out}");
        }
        out
    }

    /// The DOM view of a GPUI keystroke. GPUI reports shifted punctuation
    /// as the shifted character without the Shift flag; the US-layout table
    /// restores both, as a browser's `keyCode` tables would.
    pub fn from_keystroke(k: &Keystroke, platform: Platform) -> KeyEvent {
        let m = &k.modifiers;
        let named = match k.key.as_str() {
            "space" => Some(" "),
            "tab" => Some("Tab"),
            "enter" => Some("Enter"),
            "backspace" => Some("Backspace"),
            "escape" => Some("Escape"),
            "up" => Some("ArrowUp"),
            "down" => Some("ArrowDown"),
            "left" => Some("ArrowLeft"),
            "right" => Some("ArrowRight"),
            "pageup" => Some("PageUp"),
            "pagedown" => Some("PageDown"),
            "home" => Some("Home"),
            "end" => Some("End"),
            "delete" => Some("Delete"),
            "insert" => Some("Insert"),
            _ => None,
        };
        let fn_key = k
            .key
            .strip_prefix('f')
            .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
            .map(|d| format!("F{d}"));
        if let Some(n) = named.map(str::to_string).or(fn_key) {
            return KeyEvent {
                base: Some(n.clone()),
                shifted: Some(n.clone()),
                name: n,
                alt: m.alt,
                ctrl: m.control,
                meta: m.platform,
                shift: m.shift,
            };
        }
        let key: Option<char> = {
            let mut cs = k.key.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let Some(c) = key else {
            return KeyEvent {
                name: k.key.clone(),
                base: None,
                shifted: None,
                alt: m.alt,
                ctrl: m.control,
                meta: m.platform,
                shift: m.shift,
            };
        };
        let unshifted = SHIFT_PAIRS.iter().find(|(_, s)| *s == c).map(|(b, _)| *b);
        let shift = m.shift || unshifted.is_some();
        let base = unshifted.unwrap_or_else(|| c.to_ascii_lowercase());
        let shifted = if base.is_ascii_lowercase() {
            base.to_ascii_uppercase()
        } else {
            SHIFT_PAIRS
                .iter()
                .find(|(b, _)| *b == base)
                .map_or(base, |(_, s)| *s)
        };
        let mac_alt_typed = platform == Platform::Mac && m.alt && !m.control && !m.platform;
        let name = if mac_alt_typed {
            k.key_char.clone().unwrap_or_else(|| c.to_string())
        } else if shift {
            shifted.to_string()
        } else {
            base.to_string()
        };
        KeyEvent {
            name,
            base: Some(base.to_string()),
            shifted: Some(shifted.to_string()),
            alt: m.alt,
            ctrl: m.control,
            meta: m.platform,
            shift,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> KeyEvent {
        KeyEvent::from_keystroke(&Keystroke::parse(s).unwrap(), Platform::Mac)
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Cmd {
        Undo,
        Redo,
        Left,
        SelectLeft,
        Find,
        Toggle,
        Never,
    }

    fn keymap() -> Keymap<Cmd> {
        Keymap::build(
            &[
                KeyBinding::new("Mod-z", Cmd::Undo).prevent_default(),
                KeyBinding::new("Mod-y", Cmd::Redo).mac("Mod-Shift-z"),
                KeyBinding::new("ArrowLeft", Cmd::Left).shift(Cmd::SelectLeft),
                KeyBinding::new("Mod-f", Cmd::Find).scope("editor search-panel"),
                KeyBinding::new("Mod-Alt-c", Cmd::Toggle).scope("search-panel"),
                KeyBinding::new("Mod-/", Cmd::Never),
            ],
            Platform::Mac,
        )
    }

    fn hit(km: &Keymap<Cmd>, key: &str, scope: &str) -> Option<Cmd> {
        let mut got = None;
        km.run(&ev(key), scope, Platform::Mac, |c| {
            got = Some(*c);
            true
        });
        got
    }

    #[test]
    fn names_normalize() {
        assert_eq!(
            normalize_key_name("Mod-Shift-z", Platform::Mac),
            "Shift-Meta-z"
        );
        assert_eq!(
            normalize_key_name("Mod-Alt-g", Platform::Linux),
            "Ctrl-Alt-g"
        );
        assert_eq!(normalize_key_name("Ctrl-Space", Platform::Mac), "Ctrl- ");
        assert_eq!(normalize_key_name("Mod--", Platform::Mac), "Meta--");
    }

    #[test]
    fn resolves_like_codemirror() {
        let km = keymap();
        assert_eq!(hit(&km, "cmd-z", "editor"), Some(Cmd::Undo));
        assert_eq!(hit(&km, "cmd-shift-z", "editor"), Some(Cmd::Redo));
        assert_eq!(hit(&km, "left", "editor"), Some(Cmd::Left));
        assert_eq!(hit(&km, "shift-left", "editor"), Some(Cmd::SelectLeft));
        assert_eq!(hit(&km, "cmd-f", "search-panel"), Some(Cmd::Find));
        assert_eq!(hit(&km, "cmd-alt-c", "editor"), None);
        assert_eq!(hit(&km, "cmd-alt-c", "search-panel"), Some(Cmd::Toggle));
        assert_eq!(hit(&km, "cmd-/", "editor"), Some(Cmd::Never));
        let mut ran = 0;
        let prevented = km.run(&ev("cmd-z"), "editor", Platform::Mac, |_| {
            ran += 1;
            false
        });
        assert!(prevented);
        assert_eq!(ran, 1);
    }

    #[test]
    fn keystroke_names() {
        assert_eq!(ev("shift-a").name, "A");
        assert_eq!(ev("!").name, "!");
        assert!(ev("!").shift);
        assert_eq!(ev("cmd-[").name, "[");
        assert_eq!(ev("space").name, " ");
        assert_eq!(ev("f12").name, "F12");
    }
}
