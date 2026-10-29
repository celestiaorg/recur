//! Grammar compilation and line tokenization, a port of vscode-textmate's
//! `grammar.ts`, `tokenizeString.ts`, `basicScopesAttributeProvider.ts` and
//! `encodedTokenAttributes.ts`.
//!
//! Token offsets are UTF-8 byte offsets into the line (vscode-textmate reports
//! UTF-16 offsets; the tokens are otherwise identical).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::matcher::{Matcher, create_matchers};
use crate::onig_lib::{CaptureIndex, OnigError, OnigString};
use crate::raw::{RawCaptures, RawGrammar, RawRepository, RawRule};
use crate::registry::RegistryShared;
use crate::rule::{END_RULE_ID, RegExpSource, Rule, RuleId, RuleKind, RuleRegistry, WHILE_RULE_ID};
use crate::theme::{ScopeStack, StyleAttributes, font_style};
use crate::utils::{contains_rtl, js_parse_int};

/// Bit layout of tokenizeLine2 metadata.
pub mod meta {
    pub const LANGUAGEID_MASK: u32 = 0b0000_0000_0000_0000_0000_0000_1111_1111;
    pub const TOKEN_TYPE_MASK: u32 = 0b0000_0000_0000_0000_0000_0011_0000_0000;
    pub const BALANCED_BRACKETS_MASK: u32 = 0b0000_0000_0000_0000_0000_0100_0000_0000;
    pub const FONT_STYLE_MASK: u32 = 0b0000_0000_0000_0000_0111_1000_0000_0000;
    pub const FOREGROUND_MASK: u32 = 0b0000_0000_1111_1111_1000_0000_0000_0000;
    pub const BACKGROUND_MASK: u32 = 0b1111_1111_0000_0000_0000_0000_0000_0000;
    pub const LANGUAGEID_OFFSET: u32 = 0;
    pub const TOKEN_TYPE_OFFSET: u32 = 8;
    pub const BALANCED_BRACKETS_OFFSET: u32 = 10;
    pub const FONT_STYLE_OFFSET: u32 = 11;
    pub const FOREGROUND_OFFSET: u32 = 15;
    pub const BACKGROUND_OFFSET: u32 = 24;
}

/// `OptionalStandardTokenType.NotSet`.
const TOKEN_TYPE_NOT_SET: u32 = 8;

/// The rule-graph compile thread's stack. Only the pages it touches are committed.
const COMPILE_STACK: usize = 64 << 20;

/// `EncodedTokenAttributes.set`. Zero / `NotSet` / `None` keep the field.
pub(crate) fn set_metadata(
    existing: u32,
    language_id: u32,
    token_type: u32,
    contains_balanced_brackets: Option<bool>,
    font_style: i32,
    foreground: u32,
    background: u32,
) -> u32 {
    use meta::*;
    let mut lang = (existing & LANGUAGEID_MASK) >> LANGUAGEID_OFFSET;
    let mut tt = (existing & TOKEN_TYPE_MASK) >> TOKEN_TYPE_OFFSET;
    let mut bb = (existing & BALANCED_BRACKETS_MASK) >> BALANCED_BRACKETS_OFFSET;
    let mut fs = (existing & FONT_STYLE_MASK) >> FONT_STYLE_OFFSET;
    let mut fg = (existing & FOREGROUND_MASK) >> FOREGROUND_OFFSET;
    let mut bg = (existing & BACKGROUND_MASK) >> BACKGROUND_OFFSET;
    if language_id != 0 {
        lang = language_id;
    }
    if token_type != TOKEN_TYPE_NOT_SET {
        tt = token_type;
    }
    if let Some(b) = contains_balanced_brackets {
        bb = u32::from(b);
    }
    if font_style != font_style::NOT_SET {
        fs = font_style as u32;
    }
    if foreground != 0 {
        fg = foreground;
    }
    if background != 0 {
        bg = background;
    }
    (lang << LANGUAGEID_OFFSET)
        | (tt << TOKEN_TYPE_OFFSET)
        | (bb << BALANCED_BRACKETS_OFFSET)
        | (fs << FONT_STYLE_OFFSET)
        | (fg << FOREGROUND_OFFSET)
        | (bg << BACKGROUND_OFFSET)
}

/// `/\b(comment|string|regex|meta\.embedded)\b/` -> standard token type.
fn standard_token_type(scope: &str) -> u32 {
    let b = scope.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    for i in 0..b.len() {
        if i > 0 && word(b[i - 1]) {
            continue;
        }
        for (lit, tt) in [
            ("comment", 1),
            ("string", 2),
            ("regex", 3),
            ("meta.embedded", 0),
        ] {
            if b[i..].starts_with(lit.as_bytes()) {
                let end = i + lit.len();
                if end == b.len() || !word(b[end]) {
                    return tt;
                }
            }
        }
    }
    TOKEN_TYPE_NOT_SET
}

/// A scope list with resolved token metadata at each level.
#[derive(Clone)]
pub(crate) struct AttributedScopeStack(Arc<AttrNode>);

struct AttrNode {
    parent: Option<AttributedScopeStack>,
    scope_path: ScopeStack,
    token_attributes: u32,
}

impl AttributedScopeStack {
    fn create_root(scope_name: &str, token_attributes: u32) -> AttributedScopeStack {
        AttributedScopeStack(Arc::new(AttrNode {
            parent: None,
            scope_path: ScopeStack::new(None, scope_name),
            token_attributes,
        }))
    }

    fn create_root_and_look_up_scope_name(
        scope_name: &str,
        token_attributes: u32,
        g: &Grammar,
    ) -> AttributedScopeStack {
        let token_type = g.token_type_for_scope(scope_name);
        let scope_path = ScopeStack::new(None, scope_name);
        let style = g.theme_match(&scope_path);
        AttributedScopeStack(Arc::new(AttrNode {
            parent: None,
            token_attributes: merge_attributes(token_attributes, token_type, style),
            scope_path,
        }))
    }

    fn scope_name(&self) -> &str {
        self.0.scope_path.scope_name()
    }

    pub fn token_attributes(&self) -> u32 {
        self.0.token_attributes
    }

    pub fn scope_names(&self) -> Vec<String> {
        self.0.scope_path.segments()
    }

    fn ptr_eq(a: &AttributedScopeStack, b: &AttributedScopeStack) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    fn equals(a: Option<&AttributedScopeStack>, b: Option<&AttributedScopeStack>) -> bool {
        let (mut a, mut b) = (a, b);
        loop {
            match (a, b) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if Self::ptr_eq(x, y) {
                        return true;
                    }
                    if x.scope_name() != y.scope_name()
                        || x.token_attributes() != y.token_attributes()
                    {
                        return false;
                    }
                    a = x.0.parent.as_ref();
                    b = y.0.parent.as_ref();
                }
                _ => return false,
            }
        }
    }

    fn push_attributed(&self, scope_path: Option<&str>, g: &Grammar) -> AttributedScopeStack {
        let Some(path) = scope_path else {
            return self.clone();
        };
        if !path.contains(' ') {
            return Self::push_one(self, path, g);
        }
        let mut result = self.clone();
        for scope in path.split(' ') {
            result = Self::push_one(&result, scope, g);
        }
        result
    }

    fn push_one(
        target: &AttributedScopeStack,
        scope_name: &str,
        g: &Grammar,
    ) -> AttributedScopeStack {
        let token_type = g.token_type_for_scope(scope_name);
        let new_path = target.0.scope_path.push(scope_name);
        let style = g.theme_match(&new_path);
        AttributedScopeStack(Arc::new(AttrNode {
            parent: Some(target.clone()),
            token_attributes: merge_attributes(target.token_attributes(), token_type, style),
            scope_path: new_path,
        }))
    }
}

fn merge_attributes(existing: u32, token_type: u32, style: Option<StyleAttributes>) -> u32 {
    let (fs, fg, bg) = match style {
        Some(s) => (s.font_style, s.foreground_id, s.background_id),
        None => (font_style::NOT_SET, 0, 0),
    };
    set_metadata(existing, 0, token_type, None, fs, fg, bg)
}

/// The rule stack carried from one line to the next (`StateStack`).
/// Immutable apart from the per-line enter/anchor positions, which
/// vscode-textmate also mutates in place.
#[derive(Clone)]
pub struct StateStack(Arc<StateNode>);

struct StateNode {
    parent: Option<StateStack>,
    rule_id: RuleId,
    enter_pos: AtomicI64,
    anchor_pos: AtomicI64,
    begin_rule_captured_eol: bool,
    end_rule: Option<Arc<str>>,
    name_scopes_list: Option<AttributedScopeStack>,
    content_name_scopes_list: Option<AttributedScopeStack>,
    depth: u32,
}

impl StateStack {
    #[allow(clippy::too_many_arguments)]
    fn new(
        parent: Option<StateStack>,
        rule_id: RuleId,
        enter_pos: i64,
        anchor_pos: i64,
        begin_rule_captured_eol: bool,
        end_rule: Option<Arc<str>>,
        name_scopes_list: Option<AttributedScopeStack>,
        content_name_scopes_list: Option<AttributedScopeStack>,
    ) -> StateStack {
        let depth = parent.as_ref().map_or(1, |p| p.0.depth + 1);
        StateStack(Arc::new(StateNode {
            parent,
            rule_id,
            enter_pos: AtomicI64::new(enter_pos),
            anchor_pos: AtomicI64::new(anchor_pos),
            begin_rule_captured_eol,
            end_rule,
            name_scopes_list,
            content_name_scopes_list,
            depth,
        }))
    }

    pub fn depth(&self) -> u32 {
        self.0.depth
    }

    /// Same rules, end rules and scopes all the way down.
    pub fn equals(&self, other: &StateStack) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        if !Self::structural_equals(Some(self), Some(other)) {
            return false;
        }
        AttributedScopeStack::equals(
            self.0.content_name_scopes_list.as_ref(),
            other.0.content_name_scopes_list.as_ref(),
        )
    }

    fn structural_equals(mut a: Option<&StateStack>, mut b: Option<&StateStack>) -> bool {
        loop {
            match (a, b) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if Arc::ptr_eq(&x.0, &y.0) {
                        return true;
                    }
                    if x.0.depth != y.0.depth
                        || x.0.rule_id != y.0.rule_id
                        || x.0.end_rule != y.0.end_rule
                    {
                        return false;
                    }
                    a = x.0.parent.as_ref();
                    b = y.0.parent.as_ref();
                }
                _ => return false,
            }
        }
    }

    fn reset(&self) {
        let mut el = Some(self);
        while let Some(e) = el {
            e.0.enter_pos.store(-1, Ordering::Relaxed);
            e.0.anchor_pos.store(-1, Ordering::Relaxed);
            el = e.0.parent.as_ref();
        }
    }

    fn pop(&self) -> Option<StateStack> {
        self.0.parent.clone()
    }

    fn safe_pop(&self) -> StateStack {
        self.0.parent.clone().unwrap_or_else(|| self.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
        rule_id: RuleId,
        enter_pos: i64,
        anchor_pos: i64,
        begin_rule_captured_eol: bool,
        end_rule: Option<Arc<str>>,
        name_scopes_list: Option<AttributedScopeStack>,
        content_name_scopes_list: Option<AttributedScopeStack>,
    ) -> StateStack {
        StateStack::new(
            Some(self.clone()),
            rule_id,
            enter_pos,
            anchor_pos,
            begin_rule_captured_eol,
            end_rule,
            name_scopes_list,
            content_name_scopes_list,
        )
    }

    fn rule(&self, g: &Grammar) -> Option<Arc<Rule>> {
        g.rule(self.0.rule_id)
    }

    fn content_scopes(&self) -> Option<&AttributedScopeStack> {
        self.0.content_name_scopes_list.as_ref()
    }

    fn with_content_name_scopes_list(&self, list: Option<AttributedScopeStack>) -> StateStack {
        let same = match (&self.0.content_name_scopes_list, &list) {
            (Some(a), Some(b)) => AttributedScopeStack::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return self.clone();
        }
        StateStack::new(
            self.0.parent.clone(),
            self.0.rule_id,
            self.0.enter_pos.load(Ordering::Relaxed),
            self.0.anchor_pos.load(Ordering::Relaxed),
            self.0.begin_rule_captured_eol,
            self.0.end_rule.clone(),
            self.0.name_scopes_list.clone(),
            list,
        )
    }

    fn with_end_rule(&self, end_rule: String) -> StateStack {
        if self.0.end_rule.as_deref() == Some(end_rule.as_str()) {
            return self.clone();
        }
        StateStack::new(
            self.0.parent.clone(),
            self.0.rule_id,
            self.0.enter_pos.load(Ordering::Relaxed),
            self.0.anchor_pos.load(Ordering::Relaxed),
            self.0.begin_rule_captured_eol,
            Some(end_rule.into()),
            self.0.name_scopes_list.clone(),
            self.0.content_name_scopes_list.clone(),
        )
    }

    fn has_same_rule_as(&self, other: &StateStack) -> bool {
        let mut el = Some(self);
        while let Some(e) = el {
            if e.0.enter_pos.load(Ordering::Relaxed) != other.0.enter_pos.load(Ordering::Relaxed) {
                break;
            }
            if e.0.rule_id == other.0.rule_id {
                return true;
            }
            el = e.0.parent.as_ref();
        }
        false
    }

    /// Scope names of the innermost content scope list (debug / tests).
    pub fn scope_names(&self) -> Vec<String> {
        self.content_scopes()
            .map(|s| s.scope_names())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for StateStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StateStack(depth={}, rule={})",
            self.0.depth, self.0.rule_id
        )
    }
}

/// A scope-list token (`IToken`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub start_index: usize,
    pub end_index: usize,
    pub scopes: Vec<String>,
}

pub struct TokenizeLineResult {
    pub tokens: Vec<Token>,
    pub rule_stack: StateStack,
}

pub struct TokenizeLineResult2 {
    /// `[start, metadata, start, metadata, ...]`.
    pub tokens: Vec<u32>,
    pub rule_stack: StateStack,
}

struct LineTokens {
    emit_binary: bool,
    tokens: Vec<Token>,
    binary: Vec<u32>,
    last_token_end_index: usize,
    merge_equal_metadata: bool,
}

impl LineTokens {
    fn new(emit_binary: bool, line_text: &str) -> LineTokens {
        LineTokens {
            emit_binary,
            tokens: Vec::new(),
            binary: Vec::new(),
            last_token_end_index: 0,
            merge_equal_metadata: !contains_rtl(line_text),
        }
    }

    fn produce(&mut self, stack: &StateStack, end_index: usize) {
        self.produce_from_scopes(stack.content_scopes(), end_index);
    }

    fn produce_from_scopes(&mut self, scopes: Option<&AttributedScopeStack>, end_index: usize) {
        if self.last_token_end_index >= end_index {
            return;
        }
        if self.emit_binary {
            let metadata = scopes.map_or(0, AttributedScopeStack::token_attributes);
            if self.merge_equal_metadata && self.binary.last() == Some(&metadata) {
                self.last_token_end_index = end_index;
                return;
            }
            self.binary.push(self.last_token_end_index as u32);
            self.binary.push(metadata);
            self.last_token_end_index = end_index;
            return;
        }
        self.tokens.push(Token {
            start_index: self.last_token_end_index,
            end_index,
            scopes: scopes
                .map(AttributedScopeStack::scope_names)
                .unwrap_or_default(),
        });
        self.last_token_end_index = end_index;
    }

    fn result(mut self, stack: &StateStack, line_length: usize) -> Vec<Token> {
        if self
            .tokens
            .last()
            .is_some_and(|t| t.start_index == line_length - 1)
        {
            self.tokens.pop();
        }
        if self.tokens.is_empty() {
            self.last_token_end_index = usize::MAX;
            self.tokens.push(Token {
                start_index: 0,
                end_index: line_length,
                scopes: stack
                    .content_scopes()
                    .map(|s| s.scope_names())
                    .unwrap_or_default(),
            });
        }
        self.tokens
    }

    fn binary_result(mut self, stack: &StateStack, line_length: usize) -> Vec<u32> {
        let n = self.binary.len();
        if n > 0 && self.binary[n - 2] as usize == line_length - 1 {
            self.binary.truncate(n - 2);
        }
        if self.binary.is_empty() {
            self.binary.push(0);
            self.binary.push(
                stack
                    .content_scopes()
                    .map_or(0, AttributedScopeStack::token_attributes),
            );
        }
        self.binary
    }
}

pub(crate) struct Injection {
    matcher: Matcher,
    pub priority: i8,
    rule_id: RuleId,
}

/// A parsed `include` value.
pub(crate) enum IncludeKind<'a> {
    Base,
    SelfRef,
    Relative(&'a str),
    TopLevel(&'a str),
    TopLevelRepository(&'a str, &'a str),
}

pub(crate) fn parse_include(include: &str) -> IncludeKind<'_> {
    match include {
        "$base" => IncludeKind::Base,
        "$self" => IncludeKind::SelfRef,
        _ => match include.find('#') {
            None => IncludeKind::TopLevel(include),
            Some(0) => IncludeKind::Relative(&include[1..]),
            Some(i) => IncludeKind::TopLevelRepository(&include[..i], &include[i + 1..]),
        },
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenizeError {
    #[error(transparent)]
    Regex(#[from] OnigError),
}

pub struct Grammar {
    root_scope_name: String,
    grammar: RawGrammar,
    root_id: OnceLock<RuleId>,
    rules: RwLock<Vec<Option<Arc<Rule>>>>,
    included_grammars: Mutex<HashMap<String, Arc<RawGrammar>>>,
    registry: Arc<RegistryShared>,
    injections: OnceLock<Vec<Injection>>,
    token_types: Mutex<HashMap<String, u32>>,
}

impl RuleRegistry for Grammar {
    fn rule(&self, id: RuleId) -> Option<Arc<Rule>> {
        self.rules
            .read()
            .unwrap()
            .get(id as usize)
            .and_then(Clone::clone)
    }
}

impl Grammar {
    pub(crate) fn new(
        scope_name: &str,
        raw: &RawGrammar,
        registry: Arc<RegistryShared>,
    ) -> Grammar {
        Grammar {
            root_scope_name: scope_name.to_string(),
            grammar: raw.init(None),
            root_id: OnceLock::new(),
            rules: RwLock::new(vec![None]),
            included_grammars: Mutex::new(HashMap::new()),
            registry,
            injections: OnceLock::new(),
            token_types: Mutex::new(HashMap::new()),
        }
    }

    pub fn scope_name(&self) -> &str {
        &self.root_scope_name
    }

    fn token_type_for_scope(&self, scope: &str) -> u32 {
        let mut types = self.token_types.lock().unwrap();
        if let Some(t) = types.get(scope) {
            return *t;
        }
        let t = standard_token_type(scope);
        types.insert(scope.to_string(), t);
        t
    }

    fn theme_match(&self, path: &ScopeStack) -> Option<StyleAttributes> {
        self.registry.theme().match_path(Some(path))
    }

    fn register_rule(&self, build: impl FnOnce() -> Rule) -> RuleId {
        let id = {
            let mut rules = self.rules.write().unwrap();
            rules.push(None);
            (rules.len() - 1) as RuleId
        };
        let rule = build();
        self.rules.write().unwrap()[id as usize] = Some(Arc::new(rule));
        id
    }

    /// `RuleFactory.getCompiledRuleId`.
    fn compiled_rule_id(&self, desc: &Arc<RawRule>, repository: &RawRepository) -> RuleId {
        if desc.id.load(Ordering::Relaxed) != 0 {
            return desc.id.load(Ordering::Relaxed);
        }
        let id = {
            let mut rules = self.rules.write().unwrap();
            rules.push(None);
            (rules.len() - 1) as RuleId
        };
        desc.id.store(id, Ordering::Relaxed);
        let rule = self.build_rule(id, desc, repository);
        self.rules.write().unwrap()[id as usize] = Some(Arc::new(rule));
        id
    }

    fn build_rule(&self, id: RuleId, desc: &Arc<RawRule>, repository: &RawRepository) -> Rule {
        if let Some(m) = desc.match_.as_deref().filter(|m| !m.is_empty()) {
            let captures = self.compile_captures(desc.captures.as_ref(), repository);
            return Rule::new(
                desc.name.clone(),
                None,
                RuleKind::Match {
                    match_: RegExpSource::new(m, i64::from(id)),
                    captures,
                    cached: Mutex::new(None),
                },
            );
        }
        let Some(begin) = desc.begin.as_deref() else {
            let repository = match &desc.repository {
                Some(own) => {
                    let mut merged = (**repository).clone();
                    merged.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Arc::new(merged)
                }
                None => repository.clone(),
            };
            let include_only;
            let patterns = match (&desc.patterns, desc.include.as_deref()) {
                (Some(p), _) => Some(p.as_slice()),
                (None, Some(inc)) if !inc.is_empty() => {
                    include_only = vec![Arc::new(RawRule {
                        include: Some(inc.to_string()),
                        ..RawRule::default()
                    })];
                    Some(include_only.as_slice())
                }
                _ => None,
            };
            let (patterns, has_missing_patterns) = self.compile_patterns(patterns, &repository);
            return Rule::new(
                desc.name.clone(),
                desc.content_name.clone(),
                RuleKind::IncludeOnly {
                    patterns,
                    has_missing_patterns,
                    cached: Mutex::new(None),
                },
            );
        };
        if let Some(w) = desc.while_.as_deref().filter(|w| !w.is_empty()) {
            let begin_captures = self.compile_captures(
                desc.begin_captures.as_ref().or(desc.captures.as_ref()),
                repository,
            );
            let while_captures = self.compile_captures(
                desc.while_captures.as_ref().or(desc.captures.as_ref()),
                repository,
            );
            let (patterns, has_missing_patterns) =
                self.compile_patterns(desc.patterns.as_deref(), repository);
            return Rule::new(
                desc.name.clone(),
                desc.content_name.clone(),
                RuleKind::BeginWhile {
                    begin: RegExpSource::new(begin, i64::from(id)),
                    begin_captures,
                    while_: RegExpSource::new(w, WHILE_RULE_ID),
                    while_captures,
                    patterns,
                    has_missing_patterns,
                    cached: Mutex::new(None),
                    cached_while: Mutex::new(None),
                },
            );
        }
        let begin_captures = self.compile_captures(
            desc.begin_captures.as_ref().or(desc.captures.as_ref()),
            repository,
        );
        let end_captures = self.compile_captures(
            desc.end_captures.as_ref().or(desc.captures.as_ref()),
            repository,
        );
        let (patterns, has_missing_patterns) =
            self.compile_patterns(desc.patterns.as_deref(), repository);
        let end = desc
            .end
            .as_deref()
            .filter(|e| !e.is_empty())
            .unwrap_or("\u{FFFF}");
        Rule::new(
            desc.name.clone(),
            desc.content_name.clone(),
            RuleKind::BeginEnd {
                begin: RegExpSource::new(begin, i64::from(id)),
                begin_captures,
                end: RegExpSource::new(end, END_RULE_ID),
                end_captures,
                apply_end_pattern_last: desc.apply_end_pattern_last,
                patterns,
                has_missing_patterns,
                cached: Mutex::new(None),
            },
        )
    }

    fn compile_captures(
        &self,
        captures: Option<&RawCaptures>,
        repository: &RawRepository,
    ) -> Vec<Option<RuleId>> {
        let Some(captures) = captures else {
            return Vec::new();
        };
        let numeric = |k: &str| js_parse_int(k);
        let max = captures
            .0
            .iter()
            .filter_map(|(k, _)| numeric(k))
            .fold(0i64, i64::max);
        let mut r: Vec<Option<RuleId>> = vec![None; (max + 1) as usize];
        for (k, cap) in &captures.0 {
            let retokenize = if cap.patterns.is_some() {
                self.compiled_rule_id(cap, repository)
            } else {
                0
            };
            let name = cap.name.clone();
            let content_name = cap.content_name.clone();
            let id = self.register_rule(|| {
                Rule::new(
                    name,
                    content_name,
                    RuleKind::Capture {
                        retokenize_captured_with_rule_id: retokenize,
                    },
                )
            });
            if let Some(n) = numeric(k)
                && n >= 0
            {
                r[n as usize] = Some(id);
            }
        }
        r
    }

    fn compile_patterns(
        &self,
        patterns: Option<&[Arc<RawRule>]>,
        repository: &RawRepository,
    ) -> (Vec<RuleId>, bool) {
        let mut r = Vec::new();
        let total = patterns.map_or(0, <[_]>::len);
        for pattern in patterns.unwrap_or(&[]) {
            let rule_id = match pattern.include.as_deref().filter(|i| !i.is_empty()) {
                Some(include) => match parse_include(include) {
                    IncludeKind::Base | IncludeKind::SelfRef => repository
                        .get(include)
                        .map(|rule| self.compiled_rule_id(rule, repository)),
                    IncludeKind::Relative(name) => repository
                        .get(name)
                        .map(|rule| self.compiled_rule_id(rule, repository)),
                    IncludeKind::TopLevel(scope) => self
                        .external_grammar(scope, Some(repository))
                        .and_then(|ext| {
                            ext.repository
                                .get("$self")
                                .map(|rule| self.compiled_rule_id(rule, &ext.repository))
                        }),
                    IncludeKind::TopLevelRepository(scope, name) => self
                        .external_grammar(scope, Some(repository))
                        .and_then(|ext| {
                            ext.repository
                                .get(name)
                                .map(|rule| self.compiled_rule_id(rule, &ext.repository))
                        }),
                },
                None => Some(self.compiled_rule_id(pattern, repository)),
            };
            let Some(rule_id) = rule_id else { continue };
            let skip = self
                .rule(rule_id)
                .and_then(|rule| {
                    rule.patterns_missing()
                        .map(|(p, missing)| missing && p.is_empty())
                })
                .unwrap_or(false);
            if !skip {
                r.push(rule_id);
            }
        }
        let has_missing = total != r.len();
        (r, has_missing)
    }

    /// `getExternalGrammar`: a private initialized copy of another grammar.
    fn external_grammar(
        &self,
        scope_name: &str,
        repository: Option<&RawRepository>,
    ) -> Option<Arc<RawGrammar>> {
        if let Some(g) = self.included_grammars.lock().unwrap().get(scope_name) {
            return Some(g.clone());
        }
        let raw = self.registry.lookup(scope_name)?;
        let base = repository.and_then(|r| r.get("$base").cloned());
        let g = Arc::new(raw.init(base));
        self.included_grammars
            .lock()
            .unwrap()
            .insert(scope_name.to_string(), g.clone());
        Some(g)
    }

    fn collect_injections(&self) -> Vec<Injection> {
        let mut result = Vec::new();
        let push = |result: &mut Vec<Injection>,
                    selector: &str,
                    rule: &Arc<RawRule>,
                    repo: &RawRepository| {
            let matchers = create_matchers(selector);
            let rule_id = self.compiled_rule_id(rule, repo);
            for m in matchers {
                result.push(Injection {
                    priority: m.priority,
                    matcher: m,
                    rule_id,
                });
            }
        };
        if let Some(inj) = &self.grammar.injections {
            for (selector, rule) in inj {
                push(&mut result, selector, rule, &self.grammar.repository);
            }
        }
        for scope in self.registry.injections(&self.root_scope_name) {
            let Some(g) = self.external_grammar(&scope, None) else {
                continue;
            };
            if let Some(selector) = &g.injection_selector {
                let as_rule = Arc::new(RawRule {
                    name: g.name.clone(),
                    patterns: g.patterns.clone(),
                    repository: Some(g.repository.clone()),
                    ..RawRule::default()
                });
                push(&mut result, selector, &as_rule, &g.repository);
            }
        }
        result.sort_by_key(|i| i.priority);
        result
    }

    fn injections(&self) -> &[Injection] {
        self.injections.get_or_init(|| self.collect_injections())
    }

    /// Compile the whole rule graph once. Rule ids are assigned in compile
    /// order and stacks compare them, so threads sharing the grammar must not
    /// compile concurrently; the `OnceLock` makes later callers wait.
    ///
    /// The compile recurses once per include level, which overflows a GCD
    /// worker's 512 KiB stack in a debug build (and the kernel then kills the
    /// process with SIGILL), so it runs on its own thread with a large stack.
    fn ensure_root(&self) -> RuleId {
        *self.root_id.get_or_init(|| {
            std::thread::scope(|s| {
                match std::thread::Builder::new()
                    .name("textmate-compile".into())
                    .stack_size(COMPILE_STACK)
                    .spawn_scoped(s, || self.compile_root())
                {
                    Ok(h) => h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)),
                    Err(_) => self.compile_root(),
                }
            })
        })
    }

    fn compile_root(&self) -> RuleId {
        let Some(self_rule) = self.grammar.repository.get("$self") else {
            return 0;
        };
        let id = self.compiled_rule_id(self_rule, &self.grammar.repository);
        self.injections();
        id
    }

    fn tokenize(
        &self,
        line_text: &str,
        prev_state: Option<&StateStack>,
        emit_binary: bool,
    ) -> Result<(usize, LineTokens, StateStack), TokenizeError> {
        let root_id = self.ensure_root();
        let (is_first_line, prev) = match prev_state {
            None => {
                let defaults = self.registry.theme().defaults();
                let default_metadata = set_metadata(
                    0,
                    0,
                    TOKEN_TYPE_NOT_SET,
                    None,
                    defaults.font_style,
                    defaults.foreground_id,
                    defaults.background_id,
                );
                let root_name = self.rule(root_id).and_then(|r| r.get_name(None, None));
                let scope_list = match root_name {
                    Some(name) => AttributedScopeStack::create_root_and_look_up_scope_name(
                        &name,
                        default_metadata,
                        self,
                    ),
                    None => AttributedScopeStack::create_root("unknown", default_metadata),
                };
                (
                    true,
                    StateStack::new(
                        None,
                        root_id,
                        -1,
                        -1,
                        false,
                        None,
                        Some(scope_list.clone()),
                        Some(scope_list),
                    ),
                )
            }
            Some(s) => {
                s.reset();
                (false, s.clone())
            }
        };
        let text = format!("{line_text}\n");
        let mut tokens = LineTokens::new(emit_binary, &text);
        let line_length = text.len();
        let onig = OnigString::new(text);
        let stack = tokenize_string(self, &onig, is_first_line, 0, prev, &mut tokens, true)?;
        Ok((line_length, tokens, stack))
    }

    /// Scope-list tokens for one line (`tokenizeLine`). `None` is the
    /// initial state.
    pub fn tokenize_line(
        &self,
        line_text: &str,
        prev: Option<&StateStack>,
    ) -> Result<TokenizeLineResult, TokenizeError> {
        let (len, tokens, stack) = self.tokenize(line_text, prev, false)?;
        Ok(TokenizeLineResult {
            tokens: tokens.result(&stack, len),
            rule_stack: stack,
        })
    }

    /// Binary tokens for one line (`tokenizeLine2`).
    pub fn tokenize_line2(
        &self,
        line_text: &str,
        prev: Option<&StateStack>,
    ) -> Result<TokenizeLineResult2, TokenizeError> {
        let (len, tokens, stack) = self.tokenize(line_text, prev, true)?;
        Ok(TokenizeLineResult2 {
            tokens: tokens.binary_result(&stack, len),
            rule_stack: stack,
        })
    }
}

struct MatchResult {
    capture_indices: Vec<CaptureIndex>,
    matched_rule_id: i64,
    priority_match: bool,
}

fn match_rule(
    g: &Grammar,
    line: &OnigString,
    is_first_line: bool,
    line_pos: usize,
    stack: &StateStack,
    anchor_position: i64,
) -> Result<Option<MatchResult>, TokenizeError> {
    let Some(rule) = stack.rule(g) else {
        return Ok(None);
    };
    let scanner = rule.compile_ag(
        g,
        stack.0.end_rule.as_deref(),
        is_first_line,
        line_pos as i64 == anchor_position,
    )?;
    Ok(scanner
        .find_next_match(line, line_pos)
        .map(|r| MatchResult {
            capture_indices: r.capture_indices,
            matched_rule_id: r.rule_id,
            priority_match: false,
        }))
}

fn match_injections(
    injections: &[Injection],
    g: &Grammar,
    line: &OnigString,
    is_first_line: bool,
    line_pos: usize,
    stack: &StateStack,
    anchor_position: i64,
) -> Result<Option<MatchResult>, TokenizeError> {
    let mut best_rating = usize::MAX;
    let mut best: Option<(Vec<CaptureIndex>, i64, i8)> = None;
    let scopes = stack
        .content_scopes()
        .map(|s| s.scope_names())
        .unwrap_or_default();
    for injection in injections {
        if !injection.matcher.matches(&scopes) {
            continue;
        }
        let Some(rule) = g.rule(injection.rule_id) else {
            continue;
        };
        let scanner =
            rule.compile_ag(g, None, is_first_line, line_pos as i64 == anchor_position)?;
        let Some(m) = scanner.find_next_match(line, line_pos) else {
            continue;
        };
        let rating = m.capture_indices[0].start;
        if rating >= best_rating {
            continue;
        }
        best_rating = rating;
        best = Some((m.capture_indices, m.rule_id, injection.priority));
        if best_rating == line_pos {
            break;
        }
    }
    Ok(
        best.map(|(capture_indices, matched_rule_id, priority)| MatchResult {
            priority_match: priority == -1,
            capture_indices,
            matched_rule_id,
        }),
    )
}

fn match_rule_or_injections(
    g: &Grammar,
    line: &OnigString,
    is_first_line: bool,
    line_pos: usize,
    stack: &StateStack,
    anchor_position: i64,
) -> Result<Option<MatchResult>, TokenizeError> {
    let match_result = match_rule(g, line, is_first_line, line_pos, stack, anchor_position)?;
    let injections = g.injections();
    if injections.is_empty() {
        return Ok(match_result);
    }
    let Some(injection_result) = match_injections(
        injections,
        g,
        line,
        is_first_line,
        line_pos,
        stack,
        anchor_position,
    )?
    else {
        return Ok(match_result);
    };
    let Some(match_result) = match_result else {
        return Ok(Some(injection_result));
    };
    let match_score = match_result.capture_indices[0].start;
    let injection_score = injection_result.capture_indices[0].start;
    if injection_score < match_score
        || (injection_result.priority_match && injection_score == match_score)
    {
        return Ok(Some(injection_result));
    }
    Ok(Some(match_result))
}

struct WhileCheckResult {
    stack: StateStack,
    line_pos: usize,
    anchor_position: i64,
    is_first_line: bool,
}

fn check_while_conditions(
    g: &Grammar,
    line: &OnigString,
    mut is_first_line: bool,
    mut line_pos: usize,
    mut stack: StateStack,
    tokens: &mut LineTokens,
) -> Result<WhileCheckResult, TokenizeError> {
    let mut anchor_position: i64 = if stack.0.begin_rule_captured_eol {
        0
    } else {
        -1
    };
    let mut while_rules: Vec<(StateStack, Arc<Rule>)> = Vec::new();
    let mut node = Some(stack.clone());
    while let Some(n) = node {
        if let Some(rule) = n.rule(g)
            && matches!(rule.kind, RuleKind::BeginWhile { .. })
        {
            while_rules.push((n.clone(), rule));
        }
        node = n.pop();
    }
    while let Some((node, rule)) = while_rules.pop() {
        let scanner = rule.compile_while_ag(
            node.0.end_rule.as_deref(),
            is_first_line,
            line_pos as i64 == anchor_position,
        )?;
        match scanner.find_next_match(line, line_pos) {
            Some(r) => {
                if r.rule_id != WHILE_RULE_ID {
                    stack = node.pop().unwrap_or(node);
                    break;
                }
                if !r.capture_indices.is_empty() {
                    let caps = &r.capture_indices;
                    tokens.produce(&node, caps[0].start);
                    if let RuleKind::BeginWhile { while_captures, .. } = &rule.kind {
                        handle_captures(
                            g,
                            line,
                            is_first_line,
                            &node,
                            tokens,
                            while_captures,
                            caps,
                        )?;
                    }
                    tokens.produce(&node, caps[0].end);
                    anchor_position = caps[0].end as i64;
                    if caps[0].end > line_pos {
                        line_pos = caps[0].end;
                        is_first_line = false;
                    }
                }
            }
            None => {
                stack = node.pop().unwrap_or(node);
                break;
            }
        }
    }
    Ok(WhileCheckResult {
        stack,
        line_pos,
        anchor_position,
        is_first_line,
    })
}

fn handle_captures(
    g: &Grammar,
    line: &OnigString,
    is_first_line: bool,
    stack: &StateStack,
    tokens: &mut LineTokens,
    captures: &[Option<RuleId>],
    capture_indices: &[CaptureIndex],
) -> Result<(), TokenizeError> {
    if captures.is_empty() {
        return Ok(());
    }
    let content = line.content();
    let len = captures.len().min(capture_indices.len());
    let mut local: Vec<(AttributedScopeStack, usize)> = Vec::new();
    let max_end = capture_indices[0].end;
    for i in 0..len {
        let Some(rule_id) = captures[i] else { continue };
        let Some(capture_rule) = g.rule(rule_id) else {
            continue;
        };
        let ci = capture_indices[i];
        if ci.is_empty() {
            continue;
        }
        if ci.start > max_end {
            break;
        }
        while let Some((scopes, end)) = local.last()
            && *end <= ci.start
        {
            tokens.produce_from_scopes(Some(scopes), *end);
            local.pop();
        }
        match local.last() {
            Some((scopes, _)) => tokens.produce_from_scopes(Some(scopes), ci.start),
            None => tokens.produce(stack, ci.start),
        }
        let retokenize = match capture_rule.kind {
            RuleKind::Capture {
                retokenize_captured_with_rule_id,
            } => retokenize_captured_with_rule_id,
            _ => 0,
        };
        if retokenize != 0 {
            let scope_name = capture_rule.get_name(Some(content), Some(capture_indices));
            let base = stack.content_scopes().cloned();
            let name_scopes = base.map(|b| b.push_attributed(scope_name.as_deref(), g));
            let content_name = capture_rule.get_content_name(content, capture_indices);
            let content_scopes = name_scopes
                .as_ref()
                .map(|n| n.push_attributed(content_name.as_deref(), g));
            let stack_clone = stack.push(
                retokenize,
                ci.start as i64,
                -1,
                false,
                None,
                name_scopes,
                content_scopes,
            );
            let sub = OnigString::new(content[..ci.end].to_string());
            tokenize_string(
                g,
                &sub,
                is_first_line && ci.start == 0,
                ci.start,
                stack_clone,
                tokens,
                false,
            )?;
            continue;
        }
        if let Some(name) = capture_rule.get_name(Some(content), Some(capture_indices)) {
            let base = match local.last() {
                Some((scopes, _)) => Some(scopes.clone()),
                None => stack.content_scopes().cloned(),
            };
            if let Some(base) = base {
                local.push((base.push_attributed(Some(&name), g), ci.end));
            }
        }
    }
    while let Some((scopes, end)) = local.pop() {
        tokens.produce_from_scopes(Some(&scopes), end);
    }
    Ok(())
}

fn tokenize_string(
    g: &Grammar,
    line: &OnigString,
    mut is_first_line: bool,
    mut line_pos: usize,
    mut stack: StateStack,
    tokens: &mut LineTokens,
    check_while: bool,
) -> Result<StateStack, TokenizeError> {
    let line_length = line.content().len();
    let mut anchor_position: i64 = -1;
    if check_while {
        let r = check_while_conditions(g, line, is_first_line, line_pos, stack, tokens)?;
        stack = r.stack;
        line_pos = r.line_pos;
        is_first_line = r.is_first_line;
        anchor_position = r.anchor_position;
    }

    loop {
        let Some(r) =
            match_rule_or_injections(g, line, is_first_line, line_pos, &stack, anchor_position)?
        else {
            tokens.produce(&stack, line_length);
            return Ok(stack);
        };
        let caps = &r.capture_indices;
        let has_advanced = !caps.is_empty() && caps[0].end > line_pos;

        if r.matched_rule_id == END_RULE_ID {
            let popped_rule = stack.rule(g);
            tokens.produce(&stack, caps[0].start);
            stack = stack.with_content_name_scopes_list(stack.0.name_scopes_list.clone());
            if let Some(rule) = &popped_rule
                && let RuleKind::BeginEnd { end_captures, .. } = &rule.kind
            {
                handle_captures(g, line, is_first_line, &stack, tokens, end_captures, caps)?;
            }
            tokens.produce(&stack, caps[0].end);
            let popped = stack.clone();
            stack = stack.pop().unwrap_or_else(|| popped.clone());
            anchor_position = popped.0.anchor_pos.load(Ordering::Relaxed);
            if !has_advanced && popped.0.enter_pos.load(Ordering::Relaxed) == line_pos as i64 {
                // The grammar pushed and popped a rule without advancing
                // (vscode-textmate#12): stay in the pushed state.
                stack = popped;
                tokens.produce(&stack, line_length);
                return Ok(stack);
            }
        } else {
            let rule_id = r.matched_rule_id as RuleId;
            let Some(rule) = g.rule(rule_id) else {
                tokens.produce(&stack, line_length);
                return Ok(stack);
            };
            tokens.produce(&stack, caps[0].start);
            let before_push = stack.clone();
            let scope_name = rule.get_name(Some(line.content()), Some(caps));
            let name_scopes = stack
                .content_scopes()
                .map(|s| s.push_attributed(scope_name.as_deref(), g));
            stack = stack.push(
                rule_id,
                line_pos as i64,
                anchor_position,
                caps[0].end == line_length,
                None,
                name_scopes.clone(),
                name_scopes.clone(),
            );
            match &rule.kind {
                RuleKind::BeginEnd {
                    begin_captures,
                    end,
                    ..
                }
                | RuleKind::BeginWhile {
                    begin_captures,
                    while_: end,
                    ..
                } => {
                    handle_captures(g, line, is_first_line, &stack, tokens, begin_captures, caps)?;
                    tokens.produce(&stack, caps[0].end);
                    anchor_position = caps[0].end as i64;
                    let content_name = rule.get_content_name(line.content(), caps);
                    let content_scopes = name_scopes
                        .as_ref()
                        .map(|n| n.push_attributed(content_name.as_deref(), g));
                    stack = stack.with_content_name_scopes_list(content_scopes);
                    if end.has_back_references
                        && let Some(resolved) =
                            rule.end_with_resolved_back_references(line.content(), caps)
                    {
                        stack = stack.with_end_rule(resolved);
                    }
                    if !has_advanced && before_push.has_same_rule_as(&stack) {
                        // Pushed the same rule without advancing.
                        stack = stack.pop().unwrap_or(stack);
                        tokens.produce(&stack, line_length);
                        return Ok(stack);
                    }
                }
                _ => {
                    let captures: &[Option<RuleId>] = match &rule.kind {
                        RuleKind::Match { captures, .. } => captures,
                        _ => &[],
                    };
                    handle_captures(g, line, is_first_line, &stack, tokens, captures, caps)?;
                    tokens.produce(&stack, caps[0].end);
                    stack = stack.pop().unwrap_or(stack);
                    if !has_advanced {
                        // Neither advancing nor pushing/popping.
                        stack = stack.safe_pop();
                        tokens.produce(&stack, line_length);
                        return Ok(stack);
                    }
                }
            }
        }
        if caps[0].end > line_pos {
            line_pos = caps[0].end;
            is_first_line = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_token_types() {
        assert_eq!(standard_token_type("comment.line"), 1);
        assert_eq!(standard_token_type("string.quoted"), 2);
        assert_eq!(standard_token_type("source.regex"), 3);
        assert_eq!(standard_token_type("meta.embedded.block"), 0);
        assert_eq!(standard_token_type("strings"), TOKEN_TYPE_NOT_SET);
        assert_eq!(standard_token_type("xcomment"), TOKEN_TYPE_NOT_SET);
        assert_eq!(standard_token_type("keyword"), TOKEN_TYPE_NOT_SET);
    }

    #[test]
    fn metadata_set_keeps_unset_fields() {
        let m = set_metadata(0, 0, TOKEN_TYPE_NOT_SET, None, 2, 9, 4);
        assert_eq!(m, (2 << 11) | (9 << 15) | (4 << 24));
        let m2 = set_metadata(m, 0, 1, None, font_style::NOT_SET, 0, 0);
        assert_eq!(m2, m | (1 << 8));
    }
}
