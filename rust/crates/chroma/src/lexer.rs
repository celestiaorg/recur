//! Chroma's lexer engine: the regex state machine, emitters, mutators, and
//! the wrapping lexers (coalesce, delegate, remap).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use super::regexp2::Regex;
use super::token::TokenType;
use super::{custom, raku, registry, xml};

/// A lexing failure (a Go panic or error inside chroma).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexError(pub String);

pub(crate) type LexResult<T> = Result<T, LexError>;

fn err<T>(msg: impl Into<String>) -> LexResult<T> {
    Err(LexError(msg.into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub ty: TokenType,
    pub value: String,
}

impl Token {
    pub(crate) fn new(ty: TokenType, value: impl Into<String>) -> Token {
        Token {
            ty,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub name: String,
    pub aliases: Vec<String>,
    pub filenames: Vec<String>,
    pub alias_filenames: Vec<String>,
    pub mime_types: Vec<String>,
    pub case_insensitive: bool,
    pub dot_all: bool,
    pub not_multiline: bool,
    pub ensure_nl: bool,
    pub priority: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct TokeniseOptions {
    pub state: String,
    pub nested: bool,
    pub ensure_lf: bool,
}

impl TokeniseOptions {
    pub(crate) fn nested(state: &str) -> TokeniseOptions {
        TokeniseOptions {
            state: state.to_string(),
            nested: true,
            ensure_lf: false,
        }
    }
}

fn default_options() -> TokeniseOptions {
    TokeniseOptions {
        state: "root".to_string(),
        nested: false,
        ensure_lf: true,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Rule {
    pub pattern: String,
    pub emitter: Option<Emitter>,
    pub mutator: Option<Mutator>,
}

pub(crate) type Rules = HashMap<String, Vec<Rule>>;

/// Chroma's Go-coded emitter functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmitterFunc {
    GoTemplateString,
    HttpHeaderBlock,
    HttpContinuousHeaderBlock,
    HttpContentBlock,
    RstCodeBlock,
    RakuQuote,
    RakuPodConfig,
    RakuPodCode,
}

/// Chroma's Go-coded mutator functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MutatorFunc {
    HaxePreProc,
}

#[derive(Debug, Clone)]
pub(crate) enum Emitter {
    Token(TokenType),
    ByGroups(Vec<Option<Emitter>>),
    ByGroupNames(Vec<(String, Option<Emitter>)>),
    Using(String),
    UsingSelf(String),
    UsingByGroup {
        sublexer_name_group: usize,
        code_group: usize,
        emitters: Vec<Option<Emitter>>,
    },
    Func(EmitterFunc),
    RakuColonPair(TokenType),
}

#[derive(Debug, Clone)]
pub(crate) enum Mutator {
    Include(String),
    Combined(Vec<String>),
    Push(Vec<String>),
    Pop(usize),
    Multi(Vec<Mutator>),
    Func(MutatorFunc),
    RakuFindBrackets(u8),
    RakuReplaceRule(Arc<raku::RakuRuleConfig>),
    RakuPopRule(Arc<raku::RakuRuleConfig>),
}

#[derive(Debug)]
pub(crate) struct CompiledRule {
    pub pattern: String,
    pub emitter: Option<Emitter>,
    pub mutator: Option<Mutator>,
    pub regex: Arc<Regex>,
}

/// Compiled rules by state. Shared and replaceable: the Raku lexer swaps
/// rules while lexing, as its Go original does.
pub(crate) type CompiledRules = HashMap<String, Arc<Vec<Arc<CompiledRule>>>>;

/// chroma's per-rule match timeout.
const MATCH_TIMEOUT: Duration = Duration::from_millis(250);

/// A regex state-machine lexer (chroma `RegexLexer`) loaded from XML.
pub(crate) struct RegexLexer {
    pub config: Config,
    source: &'static str,
    compiled: OnceLock<Result<Arc<Mutex<CompiledRules>>, LexError>>,
}

impl RegexLexer {
    pub(crate) fn from_xml(source: &'static str) -> Result<RegexLexer, String> {
        let root = xml::parse(source)?;
        Ok(RegexLexer {
            config: xml::config(&root)?,
            source,
            compiled: OnceLock::new(),
        })
    }

    /// The raw rules (chroma `Rules()`), parsed from the XML.
    pub(crate) fn raw_rules(&self) -> LexResult<Rules> {
        let root = xml::parse(self.source).map_err(LexError)?;
        let rules =
            xml::rules(&root).map_err(|e| LexError(format!("{}: {e}", self.config.name)))?;
        if !rules.contains_key("root") {
            return err("no \"root\" state");
        }
        Ok(rules)
    }

    pub(crate) fn rules(&self) -> LexResult<Arc<Mutex<CompiledRules>>> {
        self.compiled
            .get_or_init(|| self.compile().map(|r| Arc::new(Mutex::new(r))))
            .clone()
    }

    fn flags(&self) -> String {
        let mut flags = String::new();
        if !self.config.not_multiline {
            flags.push('m');
        }
        if self.config.case_insensitive {
            flags.push('i');
        }
        if self.config.dot_all {
            flags.push('s');
        }
        flags
    }

    /// chroma `maybeCompile`: compile every rule, expand includes and
    /// combined states, then validate `ByGroups` against group counts.
    fn compile(&self) -> LexResult<CompiledRules> {
        let raw = self.raw_rules()?;
        let flags = self.flags();
        let mut rules: HashMap<String, Vec<Arc<CompiledRule>>> = HashMap::new();
        let mut states: Vec<&String> = raw.keys().collect();
        states.sort();
        for state in states {
            let mut list = Vec::new();
            for (i, rule) in raw[state].iter().enumerate() {
                let mut pattern = format!("(?:{})", rule.pattern);
                if !flags.is_empty() {
                    pattern = format!("(?{flags}){pattern}");
                }
                pattern = format!("\\G{pattern}");
                let mut regex = Regex::new(&pattern).map_err(|e| {
                    LexError(format!("failed to compile rule {state}.{i}: {}", e.0))
                })?;
                regex.timeout = Some(MATCH_TIMEOUT);
                list.push(Arc::new(CompiledRule {
                    pattern: rule.pattern.clone(),
                    emitter: rule.emitter.clone(),
                    mutator: rule.mutator.clone(),
                    regex: Arc::new(regex),
                }));
            }
            rules.insert(state.clone(), list);
        }
        expand_lexer_mutators(&mut rules)?;
        for (state, list) in &rules {
            for rule in list {
                if let Some(Emitter::ByGroups(emitters)) = &rule.emitter {
                    let groups = rule.regex.group_count();
                    if groups - 1 != emitters.len() {
                        return err(format!(
                            "{}: {state}: {}: number of groups {} does not match number of emitters {}",
                            self.config.name,
                            rule.pattern,
                            groups - 1,
                            emitters.len()
                        ));
                    }
                }
            }
        }
        Ok(rules.into_iter().map(|(k, v)| (k, Arc::new(v))).collect())
    }

    pub(crate) fn tokenise(
        self: &Arc<Self>,
        options: Option<&TokeniseOptions>,
        text: &str,
    ) -> LexResult<Vec<Token>> {
        let rules = self.rules()?;
        let options = options.cloned().unwrap_or_else(default_options);
        let mut text = if options.ensure_lf {
            ensure_lf(text)
        } else {
            text.to_string()
        };
        let mut newline_added = false;
        if !options.nested && self.config.ensure_nl && !text.ends_with('\n') {
            text.push('\n');
            newline_added = true;
        }
        let mut state = LexerState {
            lexer: self.clone(),
            text: text.chars().collect(),
            pos: 0,
            rules,
            stack: vec![options.state.clone()],
            state: String::new(),
            rule: 0,
            groups: Vec::new(),
            named_groups: HashMap::new(),
            context: HashMap::new(),
            options,
            newline_added,
        };
        state.run()
    }
}

/// Expands `Include` and `Combined` until none remain (chroma restarts its
/// scan after each expansion).
fn expand_lexer_mutators(rules: &mut HashMap<String, Vec<Arc<CompiledRule>>>) -> LexResult<()> {
    loop {
        let mut found = None;
        let mut states: Vec<&String> = rules.keys().collect();
        states.sort();
        'scan: for state in states {
            for (i, rule) in rules[state].iter().enumerate() {
                if matches!(
                    rule.mutator,
                    Some(Mutator::Include(_) | Mutator::Combined(_))
                ) {
                    found = Some((state.clone(), i));
                    break 'scan;
                }
            }
        }
        let Some((state, i)) = found else {
            return Ok(());
        };
        let rule = rules[&state][i].clone();
        match &rule.mutator {
            Some(Mutator::Include(name)) => {
                let Some(included) = rules.get(name).cloned() else {
                    return err(format!("invalid include state {name:?}"));
                };
                let list = rules
                    .get_mut(&state)
                    .map(std::mem::take)
                    .unwrap_or_default();
                let mut next = list[..i].to_vec();
                next.extend(included);
                next.extend_from_slice(&list[i + 1..]);
                rules.insert(state, next);
            }
            Some(Mutator::Combined(names)) => {
                let name = format!("__combined_{}", names.join("__"));
                if !rules.contains_key(&name) {
                    let mut combined = Vec::new();
                    for s in names {
                        let Some(r) = rules.get(s) else {
                            return err(format!("invalid combine state {s:?}"));
                        };
                        combined.extend(r.iter().cloned());
                    }
                    rules.insert(name.clone(), combined);
                }
                let replaced = Arc::new(CompiledRule {
                    pattern: rule.pattern.clone(),
                    emitter: rule.emitter.clone(),
                    mutator: Some(Mutator::Push(vec![name])),
                    regex: rule.regex.clone(),
                });
                // chroma rewrites the shared rule, so every state holding it sees the push.
                for list in rules.values_mut() {
                    for r in list.iter_mut() {
                        if Arc::ptr_eq(r, &rule) {
                            *r = replaced.clone();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// replaces `\r\n` and `\r` with `\n`.
fn ensure_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The state of one tokenisation (chroma `LexerState`).
pub(crate) struct LexerState {
    pub lexer: Arc<RegexLexer>,
    pub text: Vec<char>,
    pub pos: usize,
    pub rules: Arc<Mutex<CompiledRules>>,
    pub stack: Vec<String>,
    pub state: String,
    pub rule: usize,
    pub groups: Vec<String>,
    pub named_groups: HashMap<String, String>,
    pub context: HashMap<String, Box<dyn Any + Send>>,
    pub options: TokeniseOptions,
    newline_added: bool,
}

impl LexerState {
    pub(crate) fn state_rules(&self, state: &str) -> Option<Arc<Vec<Arc<CompiledRule>>>> {
        self.rules
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(state)
            .cloned()
    }

    fn run(&mut self) -> LexResult<Vec<Token>> {
        let mut out = Vec::new();
        let end = self.text.len() - usize::from(self.newline_added);
        while self.pos < end && !self.stack.is_empty() {
            self.state = self.stack.last().cloned().unwrap_or_default();
            let Some(selected) = self.state_rules(&self.state) else {
                return err(format!("unknown state {}", self.state));
            };
            let Some((index, rule, groups, named)) = match_rules(&self.text, self.pos, &selected)
            else {
                // Pygments: a newline flagged as an error resets to the root
                // state so a single unclosed string cannot taint the rest.
                if self.text[self.pos] == '\n' && self.state != self.options.state {
                    self.stack = vec![self.options.state.clone()];
                    continue;
                }
                self.pos += 1;
                out.push(Token::new(TokenType::ERROR, self.text[self.pos - 1]));
                continue;
            };
            self.rule = index;
            self.pos += groups[0].chars().count();
            self.groups = groups;
            self.named_groups = named;
            if let Some(m) = &rule.mutator {
                mutate(m, self)?;
            }
            if let Some(e) = &rule.emitter {
                let groups = self.groups.clone();
                let tokens = emit(e, &groups, self)?;
                out.extend(tokens.into_iter().filter(|t| t.ty != TokenType::IGNORE));
            }
        }
        if self.pos != self.text.len() && self.stack.is_empty() {
            let value: String = self.text[self.pos..].iter().collect();
            self.pos = self.text.len();
            out.push(Token::new(TokenType::ERROR, value));
        }
        Ok(out)
    }

    /// chroma `LexerState.Get`.
    pub(crate) fn get<T: 'static>(&self, key: &str) -> Option<&T> {
        self.context.get(key).and_then(|v| v.downcast_ref::<T>())
    }

    /// chroma `LexerState.Set`.
    pub(crate) fn set<T: Any + Send>(&mut self, key: &str, value: T) {
        self.context.insert(key.to_string(), Box::new(value));
    }
}

type RuleMatch = (
    usize,
    Arc<CompiledRule>,
    Vec<String>,
    HashMap<String, String>,
);

fn match_rules(text: &[char], pos: usize, rules: &[Arc<CompiledRule>]) -> Option<RuleMatch> {
    for (i, rule) in rules.iter().enumerate() {
        if let Ok(Some(m)) = rule.regex.match_at(text, pos) {
            let mut groups = Vec::with_capacity(m.groups.len());
            let mut named = HashMap::new();
            for (g, (name, _)) in m.groups.iter().enumerate() {
                let value = m.text(text, g);
                named.insert(name.clone(), value.clone());
                groups.push(value);
            }
            return Some((i, rule.clone(), groups, named));
        }
    }
    None
}

pub(crate) fn mutate(m: &Mutator, state: &mut LexerState) -> LexResult<()> {
    match m {
        Mutator::Include(s) => err(format!("should never reach here Include({s:?})")),
        Mutator::Combined(s) => err(format!("should never reach here Combined({s:?})")),
        Mutator::Push(states) => {
            if states.is_empty() {
                state.stack.push(state.state.clone());
            } else {
                for s in states {
                    if s == "#pop" {
                        if state.stack.pop().is_none() {
                            return err("pop from empty stack");
                        }
                    } else {
                        state.stack.push(s.clone());
                    }
                }
            }
            Ok(())
        }
        Mutator::Pop(depth) => {
            if state.stack.is_empty() {
                return err("nothing to pop");
            }
            if *depth > state.stack.len() {
                return err("pop past the bottom of the stack");
            }
            state.stack.truncate(state.stack.len() - depth);
            Ok(())
        }
        Mutator::Multi(list) => {
            for m in list {
                mutate(m, state)?;
            }
            Ok(())
        }
        Mutator::Func(MutatorFunc::HaxePreProc) => {
            custom::haxe_pre_proc(state);
            Ok(())
        }
        Mutator::RakuFindBrackets(token) => raku::find_brackets(*token, state),
        Mutator::RakuReplaceRule(rule) => raku::replace_rule(rule, state),
        Mutator::RakuPopRule(rule) => raku::pop_rule(rule, state),
    }
}

fn error_token(groups: &[String]) -> Vec<Token> {
    vec![Token::new(
        TokenType::ERROR,
        groups.first().cloned().unwrap_or_default(),
    )]
}

pub(crate) fn emit(
    e: &Emitter,
    groups: &[String],
    state: &mut LexerState,
) -> LexResult<Vec<Token>> {
    match e {
        Emitter::Token(t) => Ok(vec![Token::new(
            *t,
            groups.first().cloned().unwrap_or_default(),
        )]),
        Emitter::ByGroups(emitters) => {
            if emitters.len() + 1 != groups.len() {
                return Ok(error_token(groups));
            }
            let mut out = Vec::new();
            for (i, group) in groups[1..].iter().enumerate() {
                if let Some(e) = &emitters[i] {
                    out.extend(emit(e, std::slice::from_ref(group), state)?);
                }
            }
            Ok(out)
        }
        Emitter::ByGroupNames(map) => {
            let lookup = |name: &str| map.iter().find(|(k, _)| k == name).map(|(_, e)| e.clone());
            let mut out = Vec::new();
            if state.named_groups.len() == 1 {
                match lookup("0") {
                    Some(Some(e)) => out.extend(emit(&e, groups, state)?),
                    Some(None) => {}
                    None => out.extend(error_token(groups)),
                }
                return Ok(out);
            }
            let regex = state
                .state_rules(&state.state)
                .and_then(|r| r.get(state.rule).map(|r| r.regex.clone()))
                .ok_or_else(|| LexError("rule vanished".into()))?;
            for i in 1..state.named_groups.len() {
                let name = regex.group_name_from_number(i as i32);
                let group = state.named_groups.get(&name).cloned().unwrap_or_default();
                match lookup(&name) {
                    Some(Some(e)) => out.extend(emit(&e, &[group], state)?),
                    Some(None) => {}
                    None => out.extend(error_token(&[group])),
                }
            }
            Ok(out)
        }
        Emitter::Using(name) => {
            let lexer =
                registry::get(name).ok_or_else(|| LexError(format!("no such lexer {name:?}")))?;
            lexer.tokenise(Some(&TokeniseOptions::nested("root")), &groups[0])
        }
        Emitter::UsingSelf(s) => {
            let lexer = state.lexer.clone();
            lexer.tokenise(Some(&TokeniseOptions::nested(s)), &groups[0])
        }
        Emitter::UsingByGroup {
            sublexer_name_group,
            code_group,
            emitters,
        } => {
            if emitters.len() + 1 != groups.len() {
                return err(
                    "UsingByGroup expects number of emitters to be the same as len(groups)-1",
                );
            }
            let sublexer = groups
                .get(*sublexer_name_group)
                .and_then(|n| registry::get(n));
            let mut out = Vec::new();
            for (i, group) in groups[1..].iter().enumerate() {
                if i + 1 == *code_group
                    && let Some(sub) = &sublexer
                {
                    out.extend(sub.tokenise(None, &groups[*code_group])?);
                    continue;
                }
                if let Some(e) = &emitters[i] {
                    out.extend(emit(e, std::slice::from_ref(group), state)?);
                }
            }
            Ok(out)
        }
        Emitter::Func(f) => custom::emit_func(*f, groups, state),
        Emitter::RakuColonPair(t) => raku::colon_pair(*t, state),
    }
}

/// Any registered lexer.
pub(crate) enum Lexer {
    Regex(Arc<RegexLexer>),
    /// A language embedded in a root language (chroma `DelegatingLexer`).
    Delegating {
        root: Arc<Lexer>,
        language: Arc<Lexer>,
    },
    /// Token types remapped by type and value (chroma `TypeRemappingLexer`).
    Remapping {
        inner: Arc<Lexer>,
        lut: HashMap<TokenType, HashMap<String, TokenType>>,
    },
    /// Markdown with YAML frontmatter.
    Markdown(Arc<Lexer>),
    /// HTTP with bodies lexed by content type.
    Http(Arc<Lexer>),
}

impl Lexer {
    pub(crate) fn config(&self) -> &Config {
        match self {
            Lexer::Regex(r) => &r.config,
            Lexer::Delegating { language, .. } => language.config(),
            Lexer::Remapping { inner, .. } | Lexer::Markdown(inner) | Lexer::Http(inner) => {
                inner.config()
            }
        }
    }

    pub(crate) fn tokenise(
        &self,
        options: Option<&TokeniseOptions>,
        text: &str,
    ) -> LexResult<Vec<Token>> {
        match self {
            Lexer::Regex(r) => r.tokenise(options, text),
            Lexer::Delegating { root, language } => delegate(root, language, options, text),
            Lexer::Remapping { inner, lut } => {
                let mut tokens = inner.tokenise(options, text)?;
                for t in &mut tokens {
                    if let Some(k) = lut.get(&t.ty)
                        && let Some(tt) = k.get(&t.value).or_else(|| k.get(""))
                    {
                        t.ty = *tt;
                    }
                }
                Ok(tokens)
            }
            Lexer::Markdown(inner) => custom::markdown_tokenise(inner, options, text),
            Lexer::Http(inner) => custom::http_tokenise(inner, options, text),
        }
    }
}

/// chroma `Coalesce`: merges runs of one token type (up to 8 KiB) and
/// drops empty tokens.
pub(crate) fn coalesce(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut prev: Option<Token> = None;
    for token in tokens {
        if token.value.is_empty() {
            continue;
        }
        match &mut prev {
            None => prev = Some(token),
            Some(p) if p.ty == token.ty && p.value.len() < 8192 => p.value.push_str(&token.value),
            Some(_) => {
                if let Some(p) = prev.replace(token) {
                    out.push(p);
                }
            }
        }
    }
    out.extend(prev);
    out
}

struct Insertion {
    start: i64,
    end: i64,
    tokens: Vec<Token>,
}

/// chroma `DelegatingLexer.Tokenise`: lex with the language lexer, lex its
/// `Other` text with the root lexer, and interleave the two.
fn delegate(
    root: &Lexer,
    language: &Lexer,
    options: Option<&TokeniseOptions>,
    text: &str,
) -> LexResult<Vec<Token>> {
    let tokens = coalesce(language.tokenise(options, text)?);
    let mut others = String::new();
    let mut insertions: Vec<Insertion> = Vec::new();
    let mut offset: i64 = 0;
    let mut last: Option<TokenType> = None;
    for t in &tokens {
        if t.ty == TokenType::OTHER {
            if last.is_some_and(|l| l != TokenType::OTHER)
                && let Some(ins) = insertions.last_mut()
            {
                ins.end = offset;
            }
            others.push_str(&t.value);
        } else {
            if last.is_none_or(|l| l == TokenType::OTHER) {
                insertions.push(Insertion {
                    start: offset,
                    end: 0,
                    tokens: Vec::new(),
                });
            }
            if let Some(ins) = insertions.last_mut() {
                ins.tokens.push(t.clone());
            }
        }
        last = Some(t.ty);
        offset += t.value.len() as i64;
    }
    if insertions.is_empty() {
        return root.tokenise(options, text);
    }
    let root_tokens = coalesce(root.tokenise(options, &others)?);
    let mut out = Vec::new();
    let mut offset: i64 = 0;
    let mut root_iter = root_tokens.into_iter();
    let mut ins_iter = insertions.into_iter();
    let mut t = root_iter.next();
    let mut i = ins_iter.next();
    while t.is_some() || i.is_some() {
        let take_insertion = match (&t, &i) {
            (None, Some(_)) => true,
            (Some(tok), Some(ins)) => ins.start < offset + tok.value.len() as i64,
            _ => false,
        };
        if take_insertion {
            let Some(ins) = i.take() else { break };
            let (l, r) = split_token(t.take(), ins.start - offset)?;
            if let Some(l) = l {
                offset += l.value.len() as i64;
                out.push(l);
            }
            out.extend(ins.tokens);
            offset += ins.end - ins.start;
            t = if r.is_none() { root_iter.next() } else { r };
            i = ins_iter.next();
        } else if let Some(tok) = t.take() {
            offset += tok.value.len() as i64;
            out.push(tok);
            t = root_iter.next();
        }
    }
    Ok(out)
}

fn split_token(t: Option<Token>, offset: i64) -> LexResult<(Option<Token>, Option<Token>)> {
    let Some(t) = t else { return Ok((None, None)) };
    if offset == 0 {
        return Ok((None, Some(t)));
    }
    if offset == t.value.len() as i64 {
        return Ok((Some(t), None));
    }
    let at = usize::try_from(offset).map_err(|_| LexError("slice bounds out of range".into()))?;
    match (t.value.get(..at), t.value.get(at..)) {
        (Some(l), Some(r)) => Ok((Some(Token::new(t.ty, l)), Some(Token::new(t.ty, r)))),
        _ => err("slice bounds out of range"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_merges_runs_and_drops_empty() {
        let toks = vec![
            Token::new(TokenType::TEXT, "a"),
            Token::new(TokenType::TEXT, ""),
            Token::new(TokenType::TEXT, "b"),
            Token::new(TokenType::KEYWORD, "if"),
        ];
        assert_eq!(
            coalesce(toks),
            vec![
                Token::new(TokenType::TEXT, "ab"),
                Token::new(TokenType::KEYWORD, "if")
            ]
        );
    }

    #[test]
    fn ensure_lf_folds_cr() {
        assert_eq!(ensure_lf("a\r\nb\rc\n"), "a\nb\nc\n");
    }
}
