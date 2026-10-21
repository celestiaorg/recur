//! The Raku lexer's Go-coded behavior: delimiter matching that rewrites
//! lexer rules while lexing, colon pairs, quoting constructs and pod.
//! Like chroma, replaced rules live in the lexer's shared rule table.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, PoisonError};

use super::lexer::{
    CompiledRule, Emitter, LexError, LexResult, LexerState, Mutator, Token, TokeniseOptions, mutate,
};
use super::regexp2::{self, Regex};
use super::registry;
use super::token::TokenType;
use crate::text::go_string;

const QUOTE: u8 = 0;
const NAME_ATTRIBUTE: u8 = 1;
const POD: u8 = 2;
const POD_FORMATTER: u8 = 3;
const POD_DECLARATION: u8 = 4;
const MATCH_REGEX: u8 = 6;
const SUBSTITUTION_REGEX: u8 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RulePosition {
    Top,
    Bottom,
}

/// chroma's `ruleReplacingConfig`.
#[derive(Debug, Clone)]
pub(crate) struct RakuRuleConfig {
    pub delimiter: Vec<char>,
    pub pattern: String,
    pub token_type: Option<Emitter>,
    pub number_of_delimiter_chars: usize,
    pub mutator: Option<Mutator>,
    pub append_mutator: Option<Mutator>,
    pub rule_position: RulePosition,
    pub state_name: String,
    pub pop: bool,
    pub pop_state: bool,
    pub push_state: bool,
}

impl RakuRuleConfig {
    fn new(state_name: &str) -> Self {
        RakuRuleConfig {
            delimiter: Vec::new(),
            pattern: String::new(),
            token_type: None,
            number_of_delimiter_chars: 0,
            mutator: None,
            append_mutator: None,
            rule_position: RulePosition::Top,
            state_name: state_name.to_string(),
            pop: false,
            pop_state: false,
            push_state: false,
        }
    }
}

static BRACKETS: LazyLock<HashMap<char, char>> = LazyLock::new(|| {
    registry::data("raku_brackets.tsv")
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(a, b)| {
            let a = char::from_u32(u32::from_str_radix(a, 16).ok()?)?;
            let b = char::from_u32(u32::from_str_radix(b, 16).ok()?)?;
            Some((a, b))
        })
        .collect()
});

fn named(state: &LexerState, key: &str) -> String {
    state.named_groups.get(key).cloned().unwrap_or_default()
}

fn oob() -> LexError {
    LexError("index out of range".into())
}

fn slice(text: &[char], from: usize, to: usize) -> LexResult<String> {
    text.get(from..to)
        .map(|s| s.iter().collect())
        .ok_or_else(oob)
}

/// chroma `indexAt`: the rune index of `substr` in `s` at or after `pos`,
/// skipping a single backslash-escaped occurrence (with chroma's byte/rune
/// mix-up on the retry kept intact).
fn index_at(s: &[char], substr: &[char], pos: usize) -> LexResult<i64> {
    let from = s.get(pos..).ok_or_else(oob)?;
    let text: String = from.iter().collect();
    let needle: String = substr.iter().collect();
    let Some(byte_idx) = text.find(&needle) else {
        return Ok(-1);
    };
    let mut idx = go_string(&text.as_bytes()[..byte_idx]).chars().count();
    if (idx > 1 && from[idx - 1] == '\\' && from[idx - 2] != '\\') || (idx == 1 && from[0] == '\\')
    {
        let inner = index_at(from, substr, idx + 1)?;
        let inner = usize::try_from(inner).map_err(|_| oob())?;
        let prefix = text.as_bytes().get(..inner).ok_or_else(oob)?;
        idx = go_string(prefix).chars().count();
    }
    Ok((idx + pos) as i64)
}

/// `:to\b|:heredoc\b` with Go RE2's ASCII word boundary.
fn has_heredoc_adverb(adverbs: &str) -> bool {
    let ascii_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    [":to", ":heredoc"].iter().any(|pat| {
        adverbs.match_indices(pat).any(|(i, m)| {
            adverbs[i + m.len()..]
                .chars()
                .next()
                .is_none_or(|c| !ascii_word(c))
        })
    })
}

/// chroma's `findBrackets` mutator.
pub(crate) fn find_brackets(token: u8, state: &mut LexerState) -> LexResult<()> {
    let mut adverbs = String::new();
    let opening_chars: Vec<char> = if token == POD {
        state
            .groups
            .get(1..5)
            .ok_or_else(oob)?
            .concat()
            .chars()
            .collect()
    } else {
        adverbs = named(state, "adverbs");
        named(state, "opening_delimiters").chars().collect()
    };
    let opening_char = *opening_chars.first().ok_or_else(oob)?;
    let mut n_chars = opening_chars.len();
    let (closing_char, closing_char_exists) = if token == POD {
        ('\0', true)
    } else {
        match BRACKETS.get(&opening_char) {
            Some(&c) => (c, true),
            None => ('\0', false),
        }
    };
    let mut closing_chars: Vec<char> = Vec::new();

    match token {
        POD_FORMATTER => {
            let formatter = match named(state, "keyword").as_str() {
                "B" => TokenType::GENERIC_STRONG,
                "I" => TokenType::GENERIC_EMPH,
                "U" => TokenType::GENERIC_UNDERLINE,
                _ => TokenType::LITERAL_STRING_OTHER,
            };
            let formatter_rule = RakuRuleConfig {
                pattern: ".+?".to_string(),
                token_type: Some(Emitter::Token(formatter)),
                rule_position: RulePosition::Bottom,
                ..RakuRuleConfig::new("pod-formatter")
            };
            replace_rule(&formatter_rule, state)?;
            let delimiter_rule = RakuRuleConfig {
                delimiter: vec![closing_char],
                token_type: Some(Emitter::Token(TokenType::PUNCTUATION)),
                push_state: true,
                number_of_delimiter_chars: n_chars,
                append_mutator: Some(Mutator::RakuPopRule(Arc::new(formatter_rule))),
                ..RakuRuleConfig::new("pod-formatter")
            };
            return replace_rule(&delimiter_rule, state);
        }
        MATCH_REGEX => {
            let delimiter = if closing_char_exists {
                vec![closing_char]
            } else {
                opening_chars
            };
            let rule = RakuRuleConfig {
                delimiter,
                token_type: Some(Emitter::Token(TokenType::PUNCTUATION)),
                pop_state: true,
                push_state: true,
                ..RakuRuleConfig::new("regex")
            };
            return replace_rule(&rule, state);
        }
        SUBSTITUTION_REGEX => {
            let delimiter = regexp2::escape(&opening_chars.iter().collect::<String>());
            let rule = RakuRuleConfig {
                pattern: format!("({delimiter})((?:\\\\\\\\|\\\\/|.)*?)({delimiter})"),
                token_type: Some(Emitter::ByGroups(vec![
                    Some(Emitter::Token(TokenType::PUNCTUATION)),
                    Some(Emitter::UsingSelf("qq".to_string())),
                    Some(Emitter::Token(TokenType::PUNCTUATION)),
                ])),
                rule_position: RulePosition::Top,
                pop_state: true,
                push_state: true,
                ..RakuRuleConfig::new("regex")
            };
            return replace_rule(&rule, state);
        }
        _ => {}
    }

    let text = state.text.clone();
    let mut end_pos: i64;
    if !closing_char_exists {
        closing_chars.clone_from(&opening_chars);
        end_pos = index_at(&text, &closing_chars, state.pos)?;
    } else {
        let pod_regex = if token == POD {
            let pattern = format!(
                "{}=end\\s+{}",
                named(state, "ws"),
                regexp2::escape(&named(state, "name"))
            );
            Some(
                Regex::new(&pattern)
                    .map_err(|e| LexError(format!("regexp2: Compile({pattern:?}): {}", e.0)))?,
            )
        } else {
            closing_chars = std::iter::repeat_n(closing_char, n_chars).collect();
            None
        };
        let mut nesting_level = 1;
        let mut search_pos = state.pos as i64 - n_chars as i64;
        let mut next_close_pos: i64 = 0;
        while nesting_level > 0 {
            let from = usize::try_from(search_pos + n_chars as i64).map_err(|_| oob())?;
            if let Some(re) = &pod_regex {
                match re.find_from(&text, from) {
                    Ok(Some(m)) => {
                        let (start, len) = m.groups[0].1.ok_or_else(oob)?;
                        closing_chars = text[start..start + len].to_vec();
                        next_close_pos = m.index as i64;
                    }
                    // chroma dereferences the missing match.
                    Ok(None) => {
                        return Err(LexError(
                            "invalid memory address or nil pointer dereference".into(),
                        ));
                    }
                    Err(_) => next_close_pos = -1,
                }
            } else {
                next_close_pos = index_at(&text, &closing_chars, from)?;
            }
            let next_open_pos = index_at(&text, &opening_chars, from)?;
            if next_close_pos == -1 {
                next_close_pos = text.len() as i64;
                nesting_level = 0;
            } else if next_open_pos != -1 && next_open_pos < next_close_pos {
                nesting_level += 1;
                n_chars = opening_chars.len();
                search_pos = next_open_pos;
            } else {
                nesting_level -= 1;
                n_chars = closing_chars.len();
                search_pos = next_close_pos;
            }
        }
        end_pos = next_close_pos;
    }

    if end_pos < 0 {
        end_pos = text.len() as i64;
    }
    let mut end = end_pos as usize;

    let mut heredoc_terminator: Vec<char> = Vec::new();
    let mut end_heredoc_pos: i64 = 0;
    if has_heredoc_adverb(&adverbs) {
        if end != text.len() {
            heredoc_terminator = text.get(state.pos..end).ok_or_else(oob)?.to_vec();
            n_chars = heredoc_terminator.len();
        } else {
            end = state.pos + 1;
            n_chars = 0;
        }
        if n_chars > 0 {
            let tail = text.get(end..).ok_or_else(oob)?;
            end_heredoc_pos = index_at(tail, &heredoc_terminator, 0)?;
            if end_heredoc_pos > -1 {
                end += end_heredoc_pos as usize;
            } else {
                end = text.len();
            }
        }
    }

    let between = slice(&text, state.pos, end)?;
    let closing: String = closing_chars.iter().collect();
    let opening: String = opening_chars.iter().collect();
    match token {
        POD | POD_DECLARATION | NAME_ATTRIBUTE => {
            state.named_groups.insert("value".into(), between);
            state
                .named_groups
                .insert("closing_delimiters".into(), closing);
        }
        QUOTE => {
            if !heredoc_terminator.is_empty() {
                let first_punct = n_chars + opening_chars.len() + 1;
                let lead = slice(&text, state.pos, state.pos + first_punct)?;
                state
                    .named_groups
                    .insert("opening_delimiters".into(), format!("{opening}{lead}"));
                state
                    .named_groups
                    .insert("value".into(), slice(&text, state.pos + first_punct, end)?);
                if end_heredoc_pos > -1 {
                    state.named_groups.insert(
                        "closing_delimiters".into(),
                        heredoc_terminator.iter().collect(),
                    );
                }
            } else {
                state.named_groups.insert("value".into(), between);
                if n_chars > 0 {
                    state
                        .named_groups
                        .insert("closing_delimiters".into(), closing);
                }
            }
        }
        _ => {
            let rest = slice(&text, state.pos, end + n_chars)?;
            let first = state.groups.first().cloned().unwrap_or_default();
            state.groups = vec![first + &rest];
        }
    }
    state.pos = end + n_chars;
    Ok(())
}

fn stack_name(rule: &RakuRuleConfig) -> String {
    match rule.rule_position {
        RulePosition::Top => format!("{}-top-stack", rule.state_name),
        RulePosition::Bottom => format!("{}-bottom-stack", rule.state_name),
    }
}

/// chroma's `popRule` mutator: restores the previous replacement.
pub(crate) fn pop_rule(rule: &RakuRuleConfig, state: &mut LexerState) -> LexResult<()> {
    let name = stack_name(rule);
    let Some(mut stack) = state.get::<Vec<RakuRuleConfig>>(&name).cloned() else {
        return Ok(());
    };
    if stack.is_empty() {
        return Ok(());
    }
    stack.pop();
    let mut last = stack.last().cloned().ok_or_else(oob)?;
    last.push_state = false;
    last.pop_state = false;
    last.pop = true;
    state.set(&name, stack);
    replace_rule(&last, state)
}

/// chroma's `replaceRule` mutator: swaps the state's top or bottom rule.
pub(crate) fn replace_rule(rule: &RakuRuleConfig, state: &mut LexerState) -> LexResult<()> {
    let name = stack_name(rule);
    let mut stack = state
        .get::<Vec<RakuRuleConfig>>(&name)
        .cloned()
        .unwrap_or_default();
    if stack.is_empty() {
        stack = vec![RakuRuleConfig {
            pattern: "\\A\\z".to_string(),
            rule_position: rule.rule_position,
            ..RakuRuleConfig::new(&rule.state_name)
        }];
        state.set(&name, stack.clone());
    }

    let pop_this = Mutator::RakuPopRule(Arc::new(rule.clone()));
    let mut mutators = match (rule.rule_position, &rule.mutator) {
        (RulePosition::Top, None) => vec![Mutator::Pop(1), pop_this],
        (RulePosition::Top, Some(m)) => vec![m.clone(), pop_this],
        (_, Some(m)) => vec![m.clone()],
        (_, None) => Vec::new(),
    };
    if let Some(m) = &rule.append_mutator {
        mutators.push(m.clone());
    }
    let mutator = (!mutators.is_empty()).then_some(Mutator::Multi(mutators));

    let compiled = make_rule(rule, mutator)?;
    {
        let mut rules = state.rules.lock().unwrap_or_else(PoisonError::into_inner);
        let list = rules.get_mut(&rule.state_name).ok_or_else(oob)?;
        let mut next = (**list).clone();
        let slot = match rule.rule_position {
            RulePosition::Top => 0,
            RulePosition::Bottom => next.len().checked_sub(1).ok_or_else(oob)?,
        };
        *next.get_mut(slot).ok_or_else(oob)? = Arc::new(compiled);
        *list = Arc::new(next);
    }
    if rule.pop_state {
        mutate(&Mutator::Pop(1), state)?;
    }
    if rule.push_state {
        mutate(&Mutator::Push(vec![rule.state_name.clone()]), state)?;
    }
    if !rule.pop {
        stack.push(rule.clone());
        state.set(&name, stack);
    }
    Ok(())
}

/// chroma `makeRule`: a delimiter (escaped, not preceded by an unescaped
/// backslash) or a raw pattern, compiled without lexer flags or timeout.
fn make_rule(rule: &RakuRuleConfig, mutator: Option<Mutator>) -> LexResult<CompiledRule> {
    let pattern = if rule.delimiter.is_empty() {
        rule.pattern.clone()
    } else {
        let mut delimiter: String = rule.delimiter.iter().collect();
        if rule.number_of_delimiter_chars > 1 {
            delimiter = delimiter.repeat(rule.number_of_delimiter_chars);
        }
        format!("(?<!(?<!\\\\)\\\\){}", regexp2::escape(&delimiter))
    };
    let regex = Regex::new(&pattern)
        .map_err(|e| LexError(format!("regexp2: Compile({pattern:?}): {}", e.0)))?;
    Ok(CompiledRule {
        pattern,
        emitter: rule.token_type.clone(),
        mutator,
        regex: Arc::new(regex),
    })
}

fn tokenise_self(state: &LexerState, token_state: &str, text: &str) -> LexResult<Vec<Token>> {
    state
        .lexer
        .clone()
        .tokenise(Some(&TokeniseOptions::nested(token_state)), text)
}

/// chroma's `colonPair` emitter.
pub(crate) fn colon_pair(token_class: TokenType, state: &mut LexerState) -> LexResult<Vec<Token>> {
    let mut out = vec![Token::new(TokenType::PUNCTUATION, named(state, "colon"))];
    let key = named(state, "key");
    if token_class == TokenType::NAME_ATTRIBUTE {
        out.push(Token::new(TokenType::NAME_ATTRIBUTE, key));
    } else {
        let key_state = if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
            "common"
        } else {
            "Q"
        };
        out.extend(tokenise_self(state, key_state, &key)?);
    }
    let opening = named(state, "opening_delimiters");
    out.push(Token::new(TokenType::PUNCTUATION, opening.clone()));
    let value_state = match opening.as_str() {
        "(" | "{" | "[" => Some("root"),
        "<<" | "«" => Some("ww"),
        "<" => Some("Q"),
        _ => None,
    };
    if let Some(s) = value_state {
        out.extend(tokenise_self(state, s, &named(state, "value"))?);
    }
    out.push(Token::new(
        TokenType::PUNCTUATION,
        named(state, "closing_delimiters"),
    ));
    Ok(out)
}

/// chroma's `quote` emitter for `q`, `qq` and `Q` constructs.
pub(crate) fn quote(state: &mut LexerState) -> LexResult<Vec<Token>> {
    let keyword = named(state, "keyword");
    let adverbs = named(state, "adverbs");
    let mut out = vec![
        Token::new(TokenType::KEYWORD, keyword.clone()),
        Token::new(TokenType::LITERAL_STRING_AFFIX, adverbs.clone()),
        Token::new(TokenType::TEXT, named(state, "ws")),
        Token::new(TokenType::PUNCTUATION, named(state, "opening_delimiters")),
    ];
    let mut states: Vec<&str> = Vec::new();
    for adverb in adverbs.split(':') {
        match adverb {
            "c" | "closure" => states.push("Q-closure"),
            "qq" => states.push("qq"),
            "ww" => states.push("ww"),
            "s" | "scalar" | "a" | "array" | "h" | "hash" | "f" | "function" => {
                states.push("Q-variable")
            }
            _ => {}
        }
    }
    let has = |s: &str| states.contains(&s);
    let token_state = if keyword == "qq" || has("qq") {
        "qq"
    } else if adverbs == "ww" || has("ww") {
        "ww"
    } else if has("Q-closure") && has("Q-variable") {
        "qq"
    } else if has("Q-closure") {
        "Q-closure"
    } else if has("Q-variable") {
        "Q-variable"
    } else {
        "Q"
    };
    out.extend(tokenise_self(state, token_state, &named(state, "value"))?);
    out.push(Token::new(
        TokenType::PUNCTUATION,
        named(state, "closing_delimiters"),
    ));
    Ok(out)
}

/// chroma's `podConfig` emitter.
pub(crate) fn pod_config(groups: &[String], state: &mut LexerState) -> LexResult<Vec<Token>> {
    tokenise_self(
        state,
        "colon-pair-attribute",
        groups.first().map(String::as_str).unwrap_or_default(),
    )
}

/// `:lang\W+(\w+)` with Go RE2's ASCII classes.
fn pod_lang(config: &str) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (i, m) in config.match_indices(":lang") {
        let rest = &config[i + m.len()..];
        let sep = rest
            .chars()
            .take_while(|&c| !word(c))
            .map(char::len_utf8)
            .sum::<usize>();
        if sep == 0 {
            continue;
        }
        let lang: String = rest[sep..].chars().take_while(|&c| word(c)).collect();
        if !lang.is_empty() {
            return lang;
        }
    }
    String::new()
}

/// chroma's `podCode` emitter: a pod code block lexed by its `:lang`.
pub(crate) fn pod_code(state: &mut LexerState) -> LexResult<Vec<Token>> {
    let groups = state.named_groups.clone();
    let tok = |t: TokenType, k: &str| Token::new(t, groups.get(k).cloned().unwrap_or_default());
    let mut out = vec![
        tok(TokenType::COMMENT, "ws"),
        tok(TokenType::KEYWORD, "keyword"),
        tok(TokenType::KEYWORD, "ws2"),
        tok(TokenType::KEYWORD, "name"),
    ];
    let config = named(state, "config");
    out.extend(pod_config(std::slice::from_ref(&config), state)?);
    match registry::get(&pod_lang(&config)) {
        Some(sub) => out.extend(sub.tokenise(None, &named(state, "value"))?),
        None => out.push(tok(TokenType::LITERAL_STRING_DOC, "value")),
    }
    out.extend([
        tok(TokenType::COMMENT, "ws3"),
        tok(TokenType::KEYWORD, "end_keyword"),
        tok(TokenType::KEYWORD, "ws4"),
        tok(TokenType::KEYWORD, "name"),
    ]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_at_skips_escaped_delimiters() {
        let s: Vec<char> = "ab\\/c/d".chars().collect();
        assert_eq!(index_at(&s, &['/'], 0), Ok(5));
        assert_eq!(index_at(&s, &['x'], 0), Ok(-1));
        assert_eq!(index_at(&s, &['c'], 2), Ok(4));
    }

    #[test]
    fn adverbs_and_lang() {
        assert!(has_heredoc_adverb(":to"));
        assert!(has_heredoc_adverb(":heredoc:q"));
        assert!(!has_heredoc_adverb(":top"));
        assert_eq!(pod_lang(":lang<raku>"), "raku");
        assert_eq!(pod_lang(":langx :lang('go')"), "go");
        assert_eq!(pod_lang(":lang"), "");
    }
}
