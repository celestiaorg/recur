//! TextMate theme resolution, a port of vscode-textmate's `theme.ts`.
//!
//! The theme's font family / size / line-height extensions are not carried:
//! Recur never reads the per-token font output they feed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::utils::is_valid_hex_color;

pub mod font_style {
    pub const NOT_SET: i32 = -1;
    pub const NONE: i32 = 0;
    pub const ITALIC: i32 = 1;
    pub const BOLD: i32 = 2;
    pub const UNDERLINE: i32 = 4;
    pub const STRIKETHROUGH: i32 = 8;
}

/// One `settings[]` entry of a raw TextMate theme (`IRawThemeSetting`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawThemeSetting {
    pub name: Option<String>,
    pub scope: ThemeScope,
    pub font_style: Option<String>,
    pub foreground: Option<String>,
    pub background: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ThemeScope {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawTheme {
    pub name: Option<String>,
    pub settings: Vec<RawThemeSetting>,
}

/// Immutable scope path, innermost scope last.
#[derive(Clone, Debug)]
pub struct ScopeStack(Rc<ScopeStackNode>);

#[derive(Debug)]
struct ScopeStackNode {
    parent: Option<ScopeStack>,
    scope_name: Rc<str>,
}

impl ScopeStack {
    pub fn new(parent: Option<ScopeStack>, scope_name: &str) -> ScopeStack {
        ScopeStack(Rc::new(ScopeStackNode {
            parent,
            scope_name: scope_name.into(),
        }))
    }

    pub fn from_segments(segments: &[&str]) -> Option<ScopeStack> {
        let mut r: Option<ScopeStack> = None;
        for s in segments {
            r = Some(ScopeStack::new(r, s));
        }
        r
    }

    pub fn push(&self, scope_name: &str) -> ScopeStack {
        ScopeStack::new(Some(self.clone()), scope_name)
    }

    pub fn parent(&self) -> Option<&ScopeStack> {
        self.0.parent.as_ref()
    }

    pub fn scope_name(&self) -> &str {
        &self.0.scope_name
    }

    pub fn segments(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut item = Some(self);
        while let Some(s) = item {
            out.push(s.scope_name().to_string());
            item = s.parent();
        }
        out.reverse();
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleAttributes {
    pub font_style: i32,
    pub foreground_id: u32,
    pub background_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedThemeRule {
    scope: String,
    parent_scopes: Option<Vec<String>>,
    index: usize,
    font_style: i32,
    foreground: Option<String>,
    background: Option<String>,
}

fn parse_theme(source: &RawTheme) -> Vec<ParsedThemeRule> {
    let mut result = Vec::new();
    for (i, entry) in source.settings.iter().enumerate() {
        let scopes: Vec<String> = match &entry.scope {
            ThemeScope::One(s) => {
                let s = s.trim_start_matches(',').trim_end_matches(',');
                s.split(',').map(str::to_string).collect()
            }
            ThemeScope::Many(v) => v.clone(),
            ThemeScope::None => vec![String::new()],
        };

        let mut font_style = font_style::NOT_SET;
        if let Some(fs) = &entry.font_style {
            font_style = font_style::NONE;
            for segment in fs.split(' ') {
                font_style |= match segment {
                    "italic" => font_style::ITALIC,
                    "bold" => font_style::BOLD,
                    "underline" => font_style::UNDERLINE,
                    "strikethrough" => font_style::STRIKETHROUGH,
                    _ => 0,
                };
            }
        }
        let foreground = entry.foreground.clone().filter(|c| is_valid_hex_color(c));
        let background = entry.background.clone().filter(|c| is_valid_hex_color(c));

        for scope in scopes {
            let scope = scope.trim();
            let segments: Vec<&str> = scope.split(' ').collect();
            let last = segments[segments.len() - 1].to_string();
            let parent_scopes = (segments.len() > 1).then(|| {
                let mut p: Vec<String> = segments[..segments.len() - 1]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                p.reverse();
                p
            });
            result.push(ParsedThemeRule {
                scope: last,
                parent_scopes,
                index: i,
                font_style,
                foreground: foreground.clone(),
                background: background.clone(),
            });
        }
    }
    result
}

fn str_arr_cmp(a: Option<&[String]>, b: Option<&[String]>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => {
            if a.len() == b.len() {
                for (x, y) in a.iter().zip(b) {
                    match x.cmp(y) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                }
                Ordering::Equal
            } else {
                a.len().cmp(&b.len())
            }
        }
    }
}

/// Colour ids. Id 0 means "unset"; the map's entry 0 is empty.
#[derive(Clone, Debug, Default)]
pub struct ColorMap {
    id_to_color: Vec<String>,
    color_to_id: HashMap<String, u32>,
}

impl ColorMap {
    fn new() -> ColorMap {
        ColorMap {
            id_to_color: vec![String::new()],
            color_to_id: HashMap::new(),
        }
    }

    fn get_id(&mut self, color: Option<&str>) -> u32 {
        let Some(color) = color else { return 0 };
        let color = color.to_uppercase();
        if let Some(&v) = self.color_to_id.get(&color) {
            return v;
        }
        let id = self.id_to_color.len() as u32;
        self.color_to_id.insert(color.clone(), id);
        self.id_to_color.push(color);
        id
    }

    pub fn colors(&self) -> Vec<String> {
        self.id_to_color.clone()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TrieRule {
    scope_depth: u32,
    parent_scopes: Rc<[String]>,
    font_style: i32,
    foreground: u32,
    background: u32,
}

impl TrieRule {
    fn accept_overwrite(
        &mut self,
        scope_depth: u32,
        font_style: i32,
        foreground: u32,
        background: u32,
    ) {
        if self.scope_depth <= scope_depth {
            self.scope_depth = scope_depth;
        }
        if font_style != font_style::NOT_SET {
            self.font_style = font_style;
        }
        if foreground != 0 {
            self.foreground = foreground;
        }
        if background != 0 {
            self.background = background;
        }
    }
}

#[derive(Clone, Debug)]
struct TrieElement {
    main_rule: TrieRule,
    rules_with_parent_scopes: Vec<TrieRule>,
    children: HashMap<String, TrieElement>,
}

fn cmp_by_specificity(a: &TrieRule, b: &TrieRule) -> std::cmp::Ordering {
    if a.scope_depth != b.scope_depth {
        return b.scope_depth.cmp(&a.scope_depth);
    }
    let (mut ai, mut bi) = (0, 0);
    loop {
        if a.parent_scopes.get(ai).is_some_and(|s| s == ">") {
            ai += 1;
        }
        if b.parent_scopes.get(bi).is_some_and(|s| s == ">") {
            bi += 1;
        }
        if ai >= a.parent_scopes.len() || bi >= b.parent_scopes.len() {
            break;
        }
        let diff = b.parent_scopes[bi].len() as i64 - a.parent_scopes[ai].len() as i64;
        if diff != 0 {
            return diff.cmp(&0);
        }
        ai += 1;
        bi += 1;
    }
    b.parent_scopes.len().cmp(&a.parent_scopes.len())
}

fn split_head(scope: &str) -> (&str, &str) {
    match scope.find('.') {
        Some(i) => (&scope[..i], &scope[i + 1..]),
        None => (scope, ""),
    }
}

impl TrieElement {
    fn new(main_rule: TrieRule, rules_with_parent_scopes: Vec<TrieRule>) -> TrieElement {
        TrieElement {
            main_rule,
            rules_with_parent_scopes,
            children: HashMap::new(),
        }
    }

    fn match_scope(&self, scope: &str) -> Vec<TrieRule> {
        if !scope.is_empty() {
            let (head, tail) = split_head(scope);
            if let Some(child) = self.children.get(head) {
                return child.match_scope(tail);
            }
        }
        let mut rules: Vec<TrieRule> = self.rules_with_parent_scopes.clone();
        rules.push(self.main_rule.clone());
        rules.sort_by(cmp_by_specificity);
        rules
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        scope_depth: u32,
        scope: &str,
        parent_scopes: Option<&[String]>,
        font_style: i32,
        foreground: u32,
        background: u32,
    ) {
        if scope.is_empty() {
            self.do_insert_here(
                scope_depth,
                parent_scopes,
                font_style,
                foreground,
                background,
            );
            return;
        }
        let (head, tail) = split_head(scope);
        let main = self.main_rule.clone();
        let with_parents = self.rules_with_parent_scopes.clone();
        let child = self
            .children
            .entry(head.to_string())
            .or_insert_with(|| TrieElement::new(main, with_parents));
        child.insert(
            scope_depth + 1,
            tail,
            parent_scopes,
            font_style,
            foreground,
            background,
        );
    }

    fn do_insert_here(
        &mut self,
        scope_depth: u32,
        parent_scopes: Option<&[String]>,
        mut font_style: i32,
        mut foreground: u32,
        mut background: u32,
    ) {
        let Some(parent_scopes) = parent_scopes else {
            self.main_rule
                .accept_overwrite(scope_depth, font_style, foreground, background);
            return;
        };
        for rule in &mut self.rules_with_parent_scopes {
            if str_arr_cmp(Some(&rule.parent_scopes), Some(parent_scopes)).is_eq() {
                rule.accept_overwrite(scope_depth, font_style, foreground, background);
                return;
            }
        }
        if font_style == font_style::NOT_SET {
            font_style = self.main_rule.font_style;
        }
        if foreground == 0 {
            foreground = self.main_rule.foreground;
        }
        if background == 0 {
            background = self.main_rule.background;
        }
        self.rules_with_parent_scopes.push(TrieRule {
            scope_depth,
            parent_scopes: parent_scopes.into(),
            font_style,
            foreground,
            background,
        });
    }
}

fn matches_scope(scope_name: &str, pattern: &str) -> bool {
    pattern == scope_name
        || (scope_name.starts_with(pattern)
            && scope_name.as_bytes().get(pattern.len()) == Some(&b'.'))
}

fn scope_path_matches_parent_scopes(
    mut scope_path: Option<&ScopeStack>,
    parent_scopes: &[String],
) -> bool {
    if parent_scopes.is_empty() {
        return true;
    }
    let mut index = 0;
    while index < parent_scopes.len() {
        let mut pattern = parent_scopes[index].as_str();
        let mut must_match = false;
        if pattern == ">" {
            if index == parent_scopes.len() - 1 {
                return false;
            }
            index += 1;
            pattern = &parent_scopes[index];
            must_match = true;
        }
        while let Some(p) = scope_path {
            if matches_scope(p.scope_name(), pattern) {
                break;
            }
            if must_match {
                return false;
            }
            scope_path = p.parent();
        }
        let Some(p) = scope_path else {
            return false;
        };
        scope_path = p.parent();
        index += 1;
    }
    true
}

#[derive(Debug)]
pub struct Theme {
    color_map: ColorMap,
    defaults: StyleAttributes,
    root: TrieElement,
    cache: RefCell<HashMap<String, Rc<Vec<TrieRule>>>>,
}

impl Theme {
    pub fn from_raw(source: Option<&RawTheme>) -> Theme {
        resolve_parsed_theme_rules(source.map(parse_theme).unwrap_or_default())
    }

    pub fn color_map(&self) -> Vec<String> {
        self.color_map.colors()
    }

    pub fn defaults(&self) -> StyleAttributes {
        self.defaults
    }

    pub fn match_path(&self, scope_path: Option<&ScopeStack>) -> Option<StyleAttributes> {
        let Some(scope_path) = scope_path else {
            return Some(self.defaults);
        };
        let name = scope_path.scope_name();
        let rules = {
            let mut cache = self.cache.borrow_mut();
            match cache.get(name) {
                Some(r) => r.clone(),
                None => {
                    let r = Rc::new(self.root.match_scope(name));
                    cache.insert(name.to_string(), r.clone());
                    r
                }
            }
        };
        let rule = rules
            .iter()
            .find(|r| scope_path_matches_parent_scopes(scope_path.parent(), &r.parent_scopes))?;
        Some(StyleAttributes {
            font_style: rule.font_style,
            foreground_id: rule.foreground,
            background_id: rule.background,
        })
    }
}

fn resolve_parsed_theme_rules(mut rules: Vec<ParsedThemeRule>) -> Theme {
    rules.sort_by(|a, b| {
        a.scope
            .cmp(&b.scope)
            .then_with(|| str_arr_cmp(a.parent_scopes.as_deref(), b.parent_scopes.as_deref()))
            .then_with(|| a.index.cmp(&b.index))
    });

    let mut default_font_style = font_style::NONE;
    let mut default_foreground = "#000000".to_string();
    let mut default_background = "#ffffff".to_string();
    let mut start = 0;
    while start < rules.len() && rules[start].scope.is_empty() {
        let incoming = &rules[start];
        if incoming.font_style != font_style::NOT_SET {
            default_font_style = incoming.font_style;
        }
        if let Some(f) = &incoming.foreground {
            default_foreground = f.clone();
        }
        if let Some(b) = &incoming.background {
            default_background = b.clone();
        }
        start += 1;
    }
    let mut colors = ColorMap::new();
    let defaults = StyleAttributes {
        font_style: default_font_style,
        foreground_id: colors.get_id(Some(&default_foreground)),
        background_id: colors.get_id(Some(&default_background)),
    };
    let mut root = TrieElement::new(
        TrieRule {
            scope_depth: 0,
            parent_scopes: Rc::from(Vec::new()),
            font_style: font_style::NOT_SET,
            foreground: 0,
            background: 0,
        },
        Vec::new(),
    );
    for rule in &rules[start..] {
        let fg = colors.get_id(rule.foreground.as_deref());
        let bg = colors.get_id(rule.background.as_deref());
        root.insert(
            0,
            &rule.scope,
            rule.parent_scopes.as_deref(),
            rule.font_style,
            fg,
            bg,
        );
    }
    Theme {
        color_map: colors,
        defaults,
        root,
        cache: RefCell::new(HashMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use font_style::*;

    fn one(scope: &str) -> ThemeScope {
        ThemeScope::One(scope.into())
    }

    fn set(
        scope: ThemeScope,
        fs: Option<&str>,
        fg: Option<&str>,
        bg: Option<&str>,
    ) -> RawThemeSetting {
        RawThemeSetting {
            name: None,
            scope,
            font_style: fs.map(Into::into),
            foreground: fg.map(Into::into),
            background: bg.map(Into::into),
        }
    }

    fn theme(settings: Vec<RawThemeSetting>) -> Theme {
        Theme::from_raw(Some(&RawTheme {
            name: None,
            settings,
        }))
    }

    fn fg_of(t: &Theme, path: &[&str]) -> Option<String> {
        let r = t.match_path(ScopeStack::from_segments(path).as_ref())?;
        Some(t.color_map()[r.foreground_id as usize].clone())
    }

    fn rule(
        scope: &str,
        parents: Option<&[&str]>,
        index: usize,
        fs: i32,
        fg: Option<&str>,
        bg: Option<&str>,
    ) -> ParsedThemeRule {
        ParsedThemeRule {
            scope: scope.into(),
            parent_scopes: parents.map(|p| p.iter().map(|s| s.to_string()).collect()),
            index,
            font_style: fs,
            foreground: fg.map(Into::into),
            background: bg.map(Into::into),
        }
    }

    // vscode-textmate src/tests/themes.test.ts
    #[test]
    fn deeper_matches_have_priority() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#100000"), Some("#200000")),
            set(
                one("punctuation.definition.string.begin.html"),
                None,
                Some("#300000"),
                None,
            ),
            set(
                one("meta.tag punctuation.definition.string"),
                None,
                Some("#400000"),
                None,
            ),
        ]);
        assert_eq!(
            fg_of(&t, &["punctuation.definition.string.begin.html"]).as_deref(),
            Some("#300000")
        );
    }

    #[test]
    fn parent_matches_have_priority() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#100000"), Some("#200000")),
            set(one("c a"), None, Some("#300000"), None),
            set(one("d a.b"), None, Some("#400000"), None),
            set(one("a"), None, Some("#500000"), None),
        ]);
        assert_eq!(fg_of(&t, &["d", "a.b"]).as_deref(), Some("#400000"));

        let t = theme(vec![
            set(ThemeScope::None, None, Some("#100000"), Some("#200000")),
            set(one("meta.tag entity"), None, Some("#300000"), None),
            set(
                one("meta.selector.css entity.name.tag"),
                None,
                Some("#400000"),
                None,
            ),
            set(one("entity"), None, Some("#500000"), None),
        ]);
        assert_eq!(
            fg_of(
                &t,
                &[
                    "text.html.cshtml",
                    "meta.tag.structure.any.html",
                    "entity.name.tag.structure.any.html"
                ]
            )
            .as_deref(),
            Some("#300000")
        );
    }

    #[test]
    fn theme_matching_can_match() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#F8F8F2"), Some("#272822")),
            set(one("source, something"), None, None, Some("#100000")),
            set(
                ThemeScope::Many(vec!["bar".into(), "baz".into()]),
                None,
                None,
                Some("#200000"),
            ),
            set(one("source.css selector bar"), Some("bold"), None, None),
            set(one("constant"), Some("italic"), Some("#300000"), None),
            set(one("constant.numeric"), None, Some("#400000"), None),
            set(one("constant.numeric.hex"), Some("bold"), None, None),
            set(
                one("constant.numeric.oct"),
                Some("bold italic underline"),
                None,
                None,
            ),
            set(one("constant.numeric.dec"), Some(""), Some("#500000"), None),
            set(one("storage.object.bar"), Some(""), Some("#600000"), None),
        ]);
        let map = t.color_map();
        let m = |path: &[&str]| {
            let r = t
                .match_path(ScopeStack::from_segments(path).as_ref())
                .unwrap();
            let c = |id: u32| (id != 0).then(|| map[id as usize].clone());
            (r.font_style, c(r.foreground_id), c(r.background_id))
        };
        let s = |x: &str| Some(x.to_string());
        assert_eq!(m(&["source"]), (NOT_SET, None, s("#100000")));
        assert_eq!(m(&["source.ts"]), (NOT_SET, None, s("#100000")));
        assert_eq!(m(&["source.tss"]), (NOT_SET, None, s("#100000")));
        assert_eq!(m(&["something"]), (NOT_SET, None, s("#100000")));
        assert_eq!(m(&["something.ts"]), (NOT_SET, None, s("#100000")));
        assert_eq!(m(&["baz"]), (NOT_SET, None, s("#200000")));
        assert_eq!(m(&["baz.tss"]), (NOT_SET, None, s("#200000")));
        assert_eq!(m(&["constant"]), (ITALIC, s("#300000"), None));
        assert_eq!(m(&["constant.string"]), (ITALIC, s("#300000"), None));
        assert_eq!(m(&["constant.numeric"]), (ITALIC, s("#400000"), None));
        assert_eq!(m(&["constant.numeric.baz"]), (ITALIC, s("#400000"), None));
        assert_eq!(m(&["constant.numeric.hex"]), (BOLD, s("#400000"), None));
        assert_eq!(m(&["constant.numeric.hex.baz"]), (BOLD, s("#400000"), None));
        assert_eq!(
            m(&["constant.numeric.oct"]),
            (ITALIC | BOLD | UNDERLINE, s("#400000"), None)
        );
        assert_eq!(m(&["constant.numeric.dec"]), (NONE, s("#500000"), None));
        assert_eq!(m(&["constant.numeric.dec.baz"]), (NONE, s("#500000"), None));
        assert_eq!(m(&["storage.object.bar"]), (NONE, s("#600000"), None));
        assert_eq!(m(&["storage.object.bar.baz"]), (NONE, s("#600000"), None));
        assert_eq!(m(&["storage.object.bart"]), (NOT_SET, None, None));
        assert_eq!(m(&["storage.object"]), (NOT_SET, None, None));
        assert_eq!(m(&["storage"]), (NOT_SET, None, None));
        assert_eq!(m(&[""]), (NOT_SET, None, None));
        assert_eq!(m(&["bazz"]), (NOT_SET, None, None));
        assert_eq!(m(&["bar"]), (NOT_SET, None, s("#200000")));
        assert_eq!(
            m(&["source.css", "selector", "bar"]),
            (BOLD, None, s("#200000"))
        );
    }

    #[test]
    fn vscode_23460() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#aec2e0"), Some("#14191f")),
            set(
                one("meta.structure.dictionary.json string.quoted.double.json"),
                None,
                Some("#FF410D"),
                None,
            ),
            set(
                one("meta.structure.dictionary.json string.quoted.double.json"),
                None,
                Some("#ffffff"),
                None,
            ),
            set(
                one("meta.structure.dictionary.value.json string.quoted.double.json"),
                None,
                Some("#FF410D"),
                None,
            ),
        ]);
        let path = [
            "source.json",
            "meta.structure.dictionary.json",
            "meta.structure.dictionary.value.json",
            "string.quoted.double.json",
        ];
        assert_eq!(fg_of(&t, &path).as_deref(), Some("#FF410D"));
    }

    #[test]
    fn parses_theme_settings() {
        let raw = RawTheme {
            name: None,
            settings: vec![
                set(ThemeScope::None, None, Some("#F8F8F2"), Some("#272822")),
                set(one("source, something"), None, None, Some("#100000")),
                set(
                    ThemeScope::Many(vec!["bar".into(), "baz".into()]),
                    None,
                    None,
                    Some("#010000"),
                ),
                set(one("source.css selector bar"), Some("bold"), None, None),
                set(one("constant"), Some("italic"), Some("#ff0000"), None),
                set(one("constant.numeric"), None, Some("#00ff00"), None),
                set(one("constant.numeric.hex"), Some("bold"), None, None),
                set(
                    one("constant.numeric.oct"),
                    Some("bold italic underline"),
                    None,
                    None,
                ),
                set(
                    one("constant.numeric.bin"),
                    Some("bold strikethrough"),
                    None,
                    None,
                ),
                set(one("constant.numeric.dec"), Some(""), Some("#0000ff"), None),
                set(one("foo"), Some(""), Some("#CFA"), None),
            ],
        };
        assert_eq!(
            parse_theme(&raw),
            vec![
                rule("", None, 0, NOT_SET, Some("#F8F8F2"), Some("#272822")),
                rule("source", None, 1, NOT_SET, None, Some("#100000")),
                rule("something", None, 1, NOT_SET, None, Some("#100000")),
                rule("bar", None, 2, NOT_SET, None, Some("#010000")),
                rule("baz", None, 2, NOT_SET, None, Some("#010000")),
                rule(
                    "bar",
                    Some(&["selector", "source.css"]),
                    3,
                    BOLD,
                    None,
                    None
                ),
                rule("constant", None, 4, ITALIC, Some("#ff0000"), None),
                rule("constant.numeric", None, 5, NOT_SET, Some("#00ff00"), None),
                rule("constant.numeric.hex", None, 6, BOLD, None, None),
                rule(
                    "constant.numeric.oct",
                    None,
                    7,
                    BOLD | ITALIC | UNDERLINE,
                    None,
                    None
                ),
                rule(
                    "constant.numeric.bin",
                    None,
                    8,
                    BOLD | STRIKETHROUGH,
                    None,
                    None
                ),
                rule("constant.numeric.dec", None, 9, NONE, Some("#0000ff"), None),
                rule("foo", None, 10, NONE, Some("#CFA"), None),
            ]
        );
    }

    #[test]
    fn str_arr_cmp_matches_upstream() {
        use std::cmp::Ordering::*;
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let c = |a: Option<Vec<String>>, b: Option<Vec<String>>| {
            str_arr_cmp(a.as_deref(), b.as_deref())
        };
        assert_eq!(c(None, None), Equal);
        assert_eq!(c(None, Some(v(&[]))), Less);
        assert_eq!(c(Some(v(&[])), None), Greater);
        assert_eq!(c(Some(v(&[])), Some(v(&["a"]))), Less);
        assert_eq!(c(Some(v(&["a", "b"])), Some(v(&["a"]))), Greater);
        assert_eq!(c(Some(v(&["a", "b"])), Some(v(&["a", "c"]))), Less);
        assert_eq!(c(Some(v(&["a", "c"])), Some(v(&["a", "b"]))), Greater);
    }

    #[test]
    fn defaults_are_resolved_and_inherited() {
        let t = Theme::from_raw(None);
        assert_eq!(
            t.defaults(),
            StyleAttributes {
                font_style: NONE,
                foreground_id: 1,
                background_id: 2
            }
        );
        assert_eq!(t.color_map(), vec!["", "#000000", "#FFFFFF"]);

        let t = theme(vec![
            set(ThemeScope::None, None, None, Some("#ff0000")),
            set(ThemeScope::None, None, Some("#00ff00"), None),
            set(ThemeScope::None, Some("bold"), None, None),
        ]);
        assert_eq!(
            t.defaults(),
            StyleAttributes {
                font_style: BOLD,
                foreground_id: 1,
                background_id: 2
            }
        );
        assert_eq!(t.color_map(), vec!["", "#00FF00", "#FF0000"]);
    }

    #[test]
    fn rules_are_inherited_down_the_trie() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#F8F8F2"), Some("#272822")),
            set(one("var"), Some("bold"), Some("#100000"), None),
            set(one("var.identifier"), None, Some("#200000"), None),
            set(one("source.css var"), Some("italic"), Some("#300000"), None),
            set(one("source.css var"), Some("underline"), None, None),
        ]);
        let m = |path: &[&str]| {
            t.match_path(ScopeStack::from_segments(path).as_ref())
                .unwrap()
        };
        let id = |c: &str| t.color_map().iter().position(|x| x == c).unwrap() as u32;
        assert_eq!(m(&["var.identifier"]).font_style, BOLD);
        assert_eq!(m(&["var.identifier"]).foreground_id, id("#200000"));
        let css = m(&["source.css", "var"]);
        assert_eq!(
            (css.font_style, css.foreground_id),
            (UNDERLINE, id("#300000"))
        );
        // The deeper main rule outranks the inherited parent-scoped one.
        let css_ident = m(&["source.css", "var.identifier"]);
        assert_eq!(
            (css_ident.font_style, css_ident.foreground_id),
            (BOLD, id("#200000"))
        );
    }

    #[test]
    fn child_combinator() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#100000"), None),
            set(one("b a"), None, Some("#200000"), None),
            set(one("b > a"), None, Some("#300000"), None),
            set(one("c > b > a"), None, Some("#400000"), None),
            set(one("a"), None, Some("#500000"), None),
        ]);
        assert_eq!(fg_of(&t, &["b", "a"]).as_deref(), Some("#300000"));
        assert_eq!(fg_of(&t, &["b", "c", "a"]).as_deref(), Some("#200000"));
        assert_eq!(fg_of(&t, &["c", "b", "a"]).as_deref(), Some("#400000"));
        assert_eq!(fg_of(&t, &["c", "b", "d", "a"]).as_deref(), Some("#200000"));
    }

    #[test]
    fn deeper_scopes_have_higher_specificity_233() {
        let t = theme(vec![
            set(ThemeScope::None, None, Some("#100000"), None),
            set(one("y.z a.b"), None, Some("#200000"), None),
            set(one("x y a.b"), None, Some("#300000"), None),
        ]);
        let m = |path: &[&str]| {
            let r = t.match_path(ScopeStack::from_segments(path).as_ref())?;
            (r.foreground_id != 0).then(|| t.color_map()[r.foreground_id as usize].clone())
        };
        assert_eq!(m(&["x", "a.b"]), None);
        assert_eq!(m(&["y", "a.b"]), None);
        assert_eq!(m(&["y.z", "a"]), None);
        assert_eq!(m(&["x", "y", "a.b"]).as_deref(), Some("#300000"));
        assert_eq!(m(&["x", "y.z", "a.b"]).as_deref(), Some("#200000"));
    }

    #[test]
    fn issue_38_ignores_invalid_colors() {
        let raw = RawTheme {
            name: None,
            settings: vec![
                set(ThemeScope::None, None, Some("#cccccc"), Some("#222222")),
                set(one("variable"), Some(""), None, None),
                set(one("variable.parameter"), Some("italic"), Some(""), None),
                set(one("support.other.variable"), Some(""), None, None),
                set(one("variable.other"), Some("normal"), Some(""), None),
                set(
                    one("variable.parameter.function.coffee"),
                    Some("italic"),
                    Some("#F9D423"),
                    None,
                ),
            ],
        };
        assert_eq!(
            parse_theme(&raw),
            vec![
                rule("", None, 0, NOT_SET, Some("#cccccc"), Some("#222222")),
                rule("variable", None, 1, NONE, None, None),
                rule("variable.parameter", None, 2, ITALIC, None, None),
                rule("support.other.variable", None, 3, NONE, None, None),
                rule("variable.other", None, 4, NONE, None, None),
                rule(
                    "variable.parameter.function.coffee",
                    None,
                    5,
                    ITALIC,
                    Some("#F9D423"),
                    None
                ),
            ]
        );
    }

    #[test]
    fn issue_35_trailing_comma_in_selector() {
        let scope = [
            "meta.at-rule.return.scss,",
            "meta.at-rule.return.scss punctuation.definition,",
            "meta.at-rule.else.scss,",
            "meta.at-rule.else.scss punctuation.definition,",
            "meta.at-rule.if.scss,",
            "meta.at-rule.if.scss punctuation.definition,",
        ]
        .join("\n");
        let raw = RawTheme {
            name: None,
            settings: vec![
                set(ThemeScope::None, None, Some("#EFEFEF"), Some("#25292C")),
                set(one(&scope), None, Some("#CC7832"), None),
            ],
        };
        let p = Some(&["meta.at-rule.return.scss"][..]);
        let e = Some(&["meta.at-rule.else.scss"][..]);
        let i = Some(&["meta.at-rule.if.scss"][..]);
        let c = Some("#CC7832");
        assert_eq!(
            parse_theme(&raw),
            vec![
                rule("", None, 0, NOT_SET, Some("#EFEFEF"), Some("#25292C")),
                rule("meta.at-rule.return.scss", None, 1, NOT_SET, c, None),
                rule("punctuation.definition", p, 1, NOT_SET, c, None),
                rule("meta.at-rule.else.scss", None, 1, NOT_SET, c, None),
                rule("punctuation.definition", e, 1, NOT_SET, c, None),
                rule("meta.at-rule.if.scss", None, 1, NOT_SET, c, None),
                rule("punctuation.definition", i, 1, NOT_SET, c, None),
            ]
        );
    }
}
