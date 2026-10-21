//! File-type icons (`lib/file-icons`): the vendored file-icons/atom database, CSS and fonts,
//! embedded from their frontend paths and matched exactly like `fileIconClass`.
//!
//! GPUI's text system refuses fonts without an `m` glyph, which rules out icon fonts, so the
//! glyph is drawn from its outline. Placement matches the `.fico` box the explorer, tab chips
//! and chat chips use: a 14px square, 14px font, centered advance, Blink baseline.

use crate::color::Color;
use crate::icon::raster_icon;
use crate::theme::Theme;
use fancy_regex::Regex;
use gpui_kit::{App, IntoElement, RenderOnce, SharedString, Styled, Window, px};
use std::collections::HashMap;
use std::sync::OnceLock;

const ICONDB: &str = include_str!("../../../../frontend/src/lib/file-icons/generated/icondb.js");
const ICONS_CSS: &str = include_str!("../../../../frontend/src/lib/file-icons/generated/icons.css");
const COLORS_CSS: &str =
    include_str!("../../../../frontend/src/lib/file-icons/generated/colors.css");
const DOTENV_CSS: &str = include_str!("../../../../frontend/src/lib/file-icons/dotenv-fix.css");
const OCTICONS_CSS: &str =
    include_str!("../../../../frontend/src/lib/file-icons/atom-octicons.css");

const FONTS: &[(&str, &[u8])] = &[
    (
        "file-icons",
        include_bytes!("../../../../frontend/src/lib/file-icons/generated/fonts/file-icons.woff2"),
    ),
    (
        "Octicons Regular",
        include_bytes!("../../../../frontend/src/lib/file-icons/generated/fonts/octicons.woff2"),
    ),
    (
        "Mfizz",
        include_bytes!("../../../../frontend/src/lib/file-icons/generated/fonts/mfixx.woff2"),
    ),
    (
        "Devicons",
        include_bytes!("../../../../frontend/src/lib/file-icons/generated/fonts/devopicons.woff2"),
    ),
    (
        "FontAwesome",
        include_bytes!("../../../../frontend/src/lib/file-icons/generated/fonts/fontawesome.woff2"),
    ),
];

/// `FALLBACK` in `lib/file-icons/index.ts`.
const FALLBACK: (&str, Option<&str>) = ("text-icon", Some("medium-blue"));

/// The class pair `fileIconClass` returns, split: `icon <icon> <color>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileIconClass {
    pub icon: String,
    pub color: Option<String>,
}

impl FileIconClass {
    /// The exact class string the web UI puts on the `<span>`.
    pub fn class_string(&self) -> String {
        match &self.color {
            Some(c) => format!("icon {} {c}", self.icon),
            None => format!("icon {}", self.icon),
        }
    }
}

struct DbIcon {
    icon: String,
    colors: [Option<String>; 2],
    matcher: Option<Regex>,
}

struct Db {
    directories: Vec<DbIcon>,
    files: Vec<DbIcon>,
}

/// `fileIconClass(name, themeKey)`: dotenv first, then the first DB match on the basename,
/// else the text icon. `light` picks the light color slot.
pub fn file_icon_class(name: &str, light: bool) -> FileIconClass {
    let slot = if light { 0 } else { 1 };
    let base = name
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(name);
    if is_dotenv(base) {
        let color = if light {
            "medium-yellow"
        } else {
            "dark-yellow"
        };
        return FileIconClass {
            icon: "dotenv-icon".into(),
            color: Some(color.into()),
        };
    }
    match match_name(base, false) {
        Some(icon) => FileIconClass {
            icon: icon.icon.clone(),
            color: icon.colors[slot].clone(),
        },
        None => FileIconClass {
            icon: FALLBACK.0.into(),
            color: FALLBACK.1.map(Into::into),
        },
    }
}

/// `/^\.env(?:\.|$)/i`.
fn is_dotenv(base: &str) -> bool {
    let lower = base.to_ascii_lowercase();
    lower == ".env" || lower.starts_with(".env.")
}

type MatchCache = HashMap<(String, bool), Option<usize>>;

fn match_name(name: &str, directory: bool) -> Option<&'static DbIcon> {
    static CACHE: OnceLock<std::sync::Mutex<MatchCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let table = if directory {
        &db().directories
    } else {
        &db().files
    };
    let key = (name.to_string(), directory);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return hit.map(|i| &table[i]);
    }
    let found = table.iter().position(|icon| {
        icon.matcher
            .as_ref()
            .is_some_and(|re| re.is_match(name).unwrap_or(false))
    });
    // The web UI caches hits only; misses are cheap to cache here and change nothing.
    if let Ok(mut c) = cache.lock() {
        c.insert(key, found);
    }
    found.map(|i| &table[i])
}

fn db() -> &'static Db {
    static DB: OnceLock<Db> = OnceLock::new();
    DB.get_or_init(|| {
        let tables = parse_icondb(ICONDB);
        let mut it = tables.into_iter();
        Db {
            directories: it.next().unwrap_or_default(),
            files: it.next().unwrap_or_default(),
        }
    })
}

/// Parse `export default [[[…icons], …], [[…icons], …]]`: for each of the two tables, the
/// icon tuples `[class, [light, dark], /regex/flags, …]`.
fn parse_icondb(src: &str) -> Vec<Vec<DbIcon>> {
    let mut p = Js {
        s: src.as_bytes(),
        i: 0,
    };
    let Some(start) = src.find('[') else {
        return Vec::new();
    };
    p.i = start;
    let Some(Value::List(root)) = p.value() else {
        return Vec::new();
    };
    root.into_iter()
        .map(|table| {
            let Value::List(mut parts) = table else {
                return Vec::new();
            };
            if parts.is_empty() {
                return Vec::new();
            }
            let Value::List(icons) = parts.swap_remove(0) else {
                return Vec::new();
            };
            icons.into_iter().filter_map(db_icon).collect()
        })
        .collect()
}

fn db_icon(v: Value) -> Option<DbIcon> {
    let Value::List(fields) = v else { return None };
    let mut f = fields.into_iter();
    let Some(Value::Str(icon)) = f.next() else {
        return None;
    };
    let colors = match f.next() {
        Some(Value::List(c)) => {
            let mut c = c.into_iter().map(|v| match v {
                Value::Str(s) => Some(s),
                _ => None,
            });
            [c.next().flatten(), c.next().flatten()]
        }
        _ => [None, None],
    };
    let matcher = match f.next() {
        Some(Value::Regex(src, flags)) => js_regex(&src, &flags),
        _ => None,
    };
    Some(DbIcon {
        icon,
        colors,
        matcher,
    })
}

/// Translate a JavaScript regex literal into `fancy-regex` syntax.
fn js_regex(src: &str, flags: &str) -> Option<Regex> {
    let mut out = String::with_capacity(src.len() + 8);
    if flags.contains('i') {
        out.push_str("(?i)");
    }
    let mut chars = src.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(n) = chars.next() else { break };
                match n {
                    // `\/` is only an escape for the literal delimiter.
                    '/' => out.push('/'),
                    'd' if in_class => out.push_str("0-9"),
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push('[');
                // JS allows `[]` / `[^]`; neither appears in the DB, but a literal `]` first
                // in a class must stay escaped for Rust.
                if chars.peek() == Some(&']') {
                    chars.next();
                    out.push_str("\\]");
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '[' if in_class => out.push_str("\\["),
            _ => out.push(c),
        }
    }
    Regex::new(&out).ok()
}

#[derive(Debug)]
enum Value {
    List(Vec<Value>),
    Str(String),
    Regex(String, String),
    Other,
}

/// Just enough of a JavaScript literal parser for `icondb.js`: arrays (with holes), strings,
/// regex literals, numbers, booleans, `null` and object literals (skipped).
struct Js<'a> {
    s: &'a [u8],
    i: usize,
}

impl Js<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        match *self.s.get(self.i)? {
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    match *self.s.get(self.i)? {
                        b']' => {
                            self.i += 1;
                            return Some(Value::List(items));
                        }
                        b',' => {
                            // A hole: `[a,,b]`.
                            items.push(Value::Other);
                            self.i += 1;
                        }
                        _ => {
                            items.push(self.value()?);
                            self.ws();
                            if self.s.get(self.i) == Some(&b',') {
                                self.i += 1;
                            }
                        }
                    }
                }
            }
            b'"' | b'\'' => {
                let q = self.s[self.i];
                self.i += 1;
                let mut out = String::new();
                while let Some(&c) = self.s.get(self.i) {
                    self.i += 1;
                    if c == q {
                        break;
                    }
                    if c == b'\\' {
                        if let Some(&n) = self.s.get(self.i) {
                            out.push(n as char);
                            self.i += 1;
                        }
                        continue;
                    }
                    out.push(c as char);
                }
                Some(Value::Str(out))
            }
            b'/' => {
                self.i += 1;
                let start = self.i;
                let mut in_class = false;
                while let Some(&c) = self.s.get(self.i) {
                    match c {
                        b'\\' => self.i += 1,
                        b'[' => in_class = true,
                        b']' => in_class = false,
                        b'/' if !in_class => break,
                        _ => {}
                    }
                    self.i += 1;
                }
                let body = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                self.i += 1;
                let fstart = self.i;
                while self.s.get(self.i).is_some_and(u8::is_ascii_alphabetic) {
                    self.i += 1;
                }
                let flags = String::from_utf8_lossy(&self.s[fstart..self.i]).into_owned();
                Some(Value::Regex(body, flags))
            }
            b'{' => {
                let mut depth = 0;
                while let Some(&c) = self.s.get(self.i) {
                    self.i += 1;
                    match c {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Some(Value::Other)
            }
            _ => {
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| !matches!(c, b',' | b']' | b'}'))
                {
                    self.i += 1;
                }
                Some(Value::Other)
            }
        }
    }
}

/// A resolved glyph: CSS font family and code point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub family: String,
    pub ch: char,
}

#[derive(Default, Clone)]
struct Decl {
    family: Option<(String, bool)>,
    content: Option<(char, bool)>,
}

/// Class → glyph, cascaded over `icons.css`, then `dotenv-fix.css`, then `atom-octicons.css`
/// (import order in `index.ts`), honoring `!important`.
fn glyphs() -> &'static HashMap<String, Glyph> {
    static MAP: OnceLock<HashMap<String, Glyph>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut decls: HashMap<String, Decl> = HashMap::new();
        for css in [ICONS_CSS, DOTENV_CSS, OCTICONS_CSS] {
            for (selectors, body) in css_rules(css) {
                for sel in selectors.split(',') {
                    let sel = sel.trim();
                    let Some(class) = sel
                        .strip_prefix('.')
                        .and_then(|s| s.strip_suffix(":before"))
                    else {
                        continue;
                    };
                    if class.contains(['.', ' ', ':', '>', '[']) {
                        continue;
                    }
                    let d = decls.entry(class.to_string()).or_default();
                    for (prop, value, important) in css_decls(&body) {
                        match prop {
                            "font-family"
                                if d.family.as_ref().is_none_or(|f| !f.1 || important) =>
                            {
                                d.family = Some((unquote(value), important));
                            }
                            "content" => {
                                if let Some(ch) = css_char(value)
                                    && d.content.as_ref().is_none_or(|c| !c.1 || important)
                                {
                                    d.content = Some((ch, important));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        decls
            .into_iter()
            .filter_map(|(k, d)| {
                Some((
                    k,
                    Glyph {
                        family: d.family?.0,
                        ch: d.content?.0,
                    },
                ))
            })
            .collect()
    })
}

/// `.medium-blue:before { color: … }` from `colors.css`.
fn colors() -> &'static HashMap<String, Color> {
    static MAP: OnceLock<HashMap<String, Color>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut out = HashMap::new();
        for (selectors, body) in css_rules(COLORS_CSS) {
            for sel in selectors.split(',') {
                let Some(class) = sel
                    .trim()
                    .strip_prefix('.')
                    .and_then(|s| s.strip_suffix(":before"))
                else {
                    continue;
                };
                for (prop, value, _) in css_decls(&body) {
                    if prop == "color"
                        && let Some(c) = Color::parse(value)
                    {
                        out.insert(class.to_string(), c);
                    }
                }
            }
        }
        out
    })
}

fn css_rules(css: &str) -> Vec<(String, String)> {
    let s = strip_comments(css);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(open) = s[i..].find('{') {
        let sel = &s[i..i + open];
        let Some(close) = s[i + open..].find('}') else {
            break;
        };
        out.push((
            sel.trim().to_string(),
            s[i + open + 1..i + open + close].to_string(),
        ));
        i += open + close + 1;
    }
    out
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn css_decls(body: &str) -> impl Iterator<Item = (&str, &str, bool)> {
    body.split(';').filter_map(|d| {
        let (prop, value) = d.split_once(':')?;
        let value = value.trim();
        let (value, important) = match value.strip_suffix("!important") {
            Some(v) => (v.trim(), true),
            None => (value, false),
        };
        Some((prop.trim(), value, important))
    })
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches(['"', '\'']).to_string()
}

/// `"\f101"` → U+F101.
fn css_char(v: &str) -> Option<char> {
    let s = v.trim().trim_matches(['"', '\'']);
    match s.strip_prefix('\\') {
        Some(hex) => char::from_u32(u32::from_str_radix(hex.trim(), 16).ok()?),
        None => s.chars().next(),
    }
}

fn font_bytes(family: &str) -> Option<&'static [u8]> {
    static DECODED: OnceLock<HashMap<&'static str, Vec<u8>>> = OnceLock::new();
    DECODED
        .get_or_init(|| {
            FONTS
                .iter()
                .filter_map(|(name, bytes)| {
                    let ttf = if bytes.starts_with(b"wOF2") {
                        wuff::decompress_woff2(bytes).ok()?
                    } else if bytes.starts_with(b"wOFF") {
                        wuff::decompress_woff1(bytes).ok()?
                    } else {
                        bytes.to_vec()
                    };
                    Some((*name, ttf))
                })
                .collect()
        })
        .get(family)
        .map(Vec::as_slice)
}

/// The glyph and color the `.fico` span shows for a class pair.
pub fn resolve(class: &FileIconClass) -> Option<(Glyph, Option<Color>)> {
    let g = glyphs().get(&class.icon)?.clone();
    let c = class.color.as_ref().and_then(|c| colors().get(c).copied());
    Some((g, c))
}

/// SVG markup of a glyph as `.fico` lays it out: a `box`×`box` CSS px square, `box` px font,
/// line-height `box`, advance centered, baseline placed as WebKit places it at `scale`.
/// Baseline origin of a `.fico` glyph in its `box` square: advance centered, baseline where
/// WebKit puts it for a `box`px font at line-height `box` (on a whole device pixel).
fn glyph_origin(
    face: &ttf_parser::Face<'_>,
    gid: ttf_parser::GlyphId,
    boxpx: f32,
    scale: f32,
) -> (f32, f32) {
    let k = boxpx / face.units_per_em() as f32;
    let ascent = face.ascender() as f32 * k;
    let descent = -(face.descender() as f32) * k;
    let m = crate::text::line_metrics(
        ascent,
        descent,
        0.,
        boxpx,
        crate::text::LineHeight::Px(boxpx),
    );
    let advance = face.glyph_hor_advance(gid).unwrap_or(0) as f32 * k;
    ((boxpx - advance) / 2., (m.baseline * scale).round() / scale)
}

/// SVG markup of a `.fico` glyph outline (the non-macOS raster path, and tests).
pub fn glyph_svg(glyph: &Glyph, color: Color, boxpx: f32, scale: f32) -> Option<String> {
    let bytes = font_bytes(&glyph.family)?;
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let gid = face.glyph_index(glyph.ch)?;
    let (x0, y0) = glyph_origin(&face, gid, boxpx, scale);
    let mut path = SvgPath {
        d: String::new(),
        x0,
        y0,
        k: boxpx / face.units_per_em() as f32,
    };
    face.outline_glyph(gid, &mut path)?;
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {boxpx} {boxpx}\"><path fill=\"{}\" d=\"{}\"/></svg>",
        color.to_css(),
        path.d
    ))
}

struct SvgPath {
    d: String,
    x0: f32,
    y0: f32,
    k: f32,
}

impl SvgPath {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x0 + x * self.k, self.y0 - y * self.k)
    }
}

impl ttf_parser::OutlineBuilder for SvgPath {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.d.push_str(&format!("M{x} {y}"));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.d.push_str(&format!("L{x} {y}"));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.d.push_str(&format!("Q{a} {b} {x} {y}"));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.d.push_str(&format!("C{a} {b} {c} {d} {x} {y}"));
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// The `.fico` file icon for a file name (`fileIconClass` + the 14px `.fico` box).
#[derive(IntoElement)]
pub struct FileIcon {
    name: SharedString,
}

/// A file icon in the current theme's color slot. Uncolored classes take the inherited text
/// color, as the CSS does.
pub fn file_icon(name: impl Into<SharedString>) -> FileIcon {
    FileIcon { name: name.into() }
}

impl RenderOnce for FileIcon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let light = Theme::of(cx).is_light();
        let fallback = Color::from_hsla(window.text_style().color);
        let class = file_icon_class(&self.name, light);
        let resolved = resolve(&class).map(|(g, c)| (g, c.unwrap_or(fallback)));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(&class, fallback.to_css()), &mut h);
        let key = std::hash::Hasher::finish(&h);
        raster_icon(key, move |w, hh, scale| {
            let (glyph, color) = resolved.as_ref()?;
            draw_glyph(glyph, *color, 14., w, hh, scale)
        })
        .size(px(14.))
    }
}

/// Rasterize a `.fico` glyph into a `w`×`h` device-pixel bitmap (premultiplied RGBA).
fn draw_glyph(
    glyph: &Glyph,
    color: Color,
    boxpx: f32,
    w: u32,
    h: u32,
    scale: f32,
) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        let bytes = font_bytes(&glyph.family)?;
        let face = ttf_parser::Face::parse(bytes, 0).ok()?;
        let gid = face.glyph_index(glyph.ch)?;
        let (x0, baseline) = glyph_origin(&face, gid, boxpx, scale);
        crate::cg_raster::render_glyph(
            bytes,
            gid.0,
            boxpx,
            (x0, baseline),
            (color.r, color.g, color.b, color.a),
            // `.icon:before { -webkit-font-smoothing: antialiased }`.
            crate::cg_raster::TextSmoothing::Antialiased,
            w,
            h,
            scale,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::icon::rasterize(
            &glyph_svg(glyph, color, boxpx, scale)?,
            (w, h),
            (0., 0.),
            (w as f32, h as f32),
            0.,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_parses_and_every_regex_compiles() {
        let db = db();
        assert!(db.files.len() > 1500, "{}", db.files.len());
        assert!(!db.directories.is_empty());
        let broken: Vec<&str> = db
            .files
            .iter()
            .chain(&db.directories)
            .filter(|i| i.matcher.is_none())
            .map(|i| i.icon.as_str())
            .collect();
        assert!(broken.is_empty(), "uncompiled: {broken:?}");
    }

    #[test]
    fn matches_like_file_icon_class() {
        assert_eq!(file_icon_class("main.rs", false).icon, "rust-icon");
        assert_eq!(file_icon_class("src/app.ts", false).icon, "ts-icon");
        assert_eq!(file_icon_class("package.json", false).icon, "npm-icon");
        assert_eq!(
            file_icon_class(".env.local", true).class_string(),
            "icon dotenv-icon medium-yellow"
        );
        assert_eq!(
            file_icon_class(".ENV", false).class_string(),
            "icon dotenv-icon dark-yellow"
        );
        assert_eq!(
            file_icon_class("zzz.unknownext", false).class_string(),
            "icon text-icon medium-blue"
        );
    }

    #[test]
    fn every_db_icon_resolves_to_a_drawable_glyph() {
        let missing: Vec<String> = db()
            .files
            .iter()
            .map(|i| i.icon.clone())
            .filter(|c| {
                let class = FileIconClass {
                    icon: c.clone(),
                    color: None,
                };
                resolve(&class).is_none()
            })
            .collect();
        assert!(missing.is_empty(), "no CSS glyph for {missing:?}");
        let (g, c) = resolve(&file_icon_class(".env", false)).unwrap();
        assert_eq!(g.family, "Octicons Regular");
        assert_eq!(g.ch, '\u{f049}');
        assert!(c.is_some());
        assert!(
            glyph_svg(&g, c.unwrap(), 14., 2.)
                .unwrap()
                .contains("<path")
        );
    }
}
