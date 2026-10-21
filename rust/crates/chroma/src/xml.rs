//! Chroma's XML lexer format, read the way Go `encoding/xml` reads it:
//! repeated attributes append, comments are ignored, `\r\n` and `\r` become
//! `\n`, and only the predefined and numeric entities are allowed.

use std::collections::HashMap;
use std::sync::Arc;

use super::lexer::{Config, Emitter, EmitterFunc, Mutator, MutatorFunc, Rule, Rules};
use super::raku::{RakuRuleConfig, RulePosition};
use super::token::TokenType;

#[derive(Debug, Clone, Default)]
pub(crate) struct Elem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Elem>,
    pub text: String,
}

impl Elem {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn attrs_named(&self, name: &str) -> Vec<String> {
        self.attrs
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn child(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.name == name)
    }

    fn first_descendant(&self, name: &str) -> Option<&Elem> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.first_descendant(name))
    }
}

/// Parses an XML document into its root element.
pub(crate) fn parse(doc: &str) -> Result<Elem, String> {
    let mut p = XmlParser {
        s: doc.as_bytes(),
        i: 0,
    };
    p.skip_prolog()?;
    let root = p.element()?;
    Ok(root)
}

struct XmlParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl XmlParser<'_> {
    fn starts_with(&self, pat: &str) -> bool {
        self.s[self.i..].starts_with(pat.as_bytes())
    }

    fn skip_until(&mut self, pat: &str) -> Result<(), String> {
        match find(&self.s[self.i..], pat.as_bytes()) {
            Some(off) => {
                self.i += off + pat.len();
                Ok(())
            }
            None => Err(format!("unterminated {pat}")),
        }
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            if self.starts_with("<!--") {
                self.skip_until("-->")?;
            } else if self.starts_with("<?") {
                self.skip_until("?>")?;
            } else {
                return Ok(());
            }
        }
    }

    fn skip_prolog(&mut self) -> Result<(), String> {
        self.skip_misc()
    }

    fn name(&mut self) -> String {
        let start = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_whitespace() || c == b'>' || c == b'/' || c == b'=' {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn element(&mut self) -> Result<Elem, String> {
        if self.s.get(self.i) != Some(&b'<') {
            return Err(format!("expected element at byte {}", self.i));
        }
        self.i += 1;
        let mut elem = Elem {
            name: self.name(),
            ..Elem::default()
        };
        loop {
            self.skip_ws();
            match self.s.get(self.i) {
                Some(b'/') => {
                    if self.s.get(self.i + 1) != Some(&b'>') {
                        return Err("malformed empty element".into());
                    }
                    self.i += 2;
                    return Ok(elem);
                }
                Some(b'>') => {
                    self.i += 1;
                    break;
                }
                Some(_) => {
                    let key = self.name();
                    self.skip_ws();
                    if self.s.get(self.i) != Some(&b'=') {
                        return Err(format!("attribute {key} without value"));
                    }
                    self.i += 1;
                    self.skip_ws();
                    let quote = *self.s.get(self.i).ok_or("unexpected end")?;
                    if quote != b'"' && quote != b'\'' {
                        return Err("unquoted attribute".into());
                    }
                    self.i += 1;
                    let start = self.i;
                    while self.i < self.s.len() && self.s[self.i] != quote {
                        self.i += 1;
                    }
                    let raw = &self.s[start..self.i];
                    self.i += 1;
                    elem.attrs.push((key, decode_text(raw)?));
                }
                None => return Err("unexpected end in tag".into()),
            }
        }
        // Content.
        loop {
            if self.i >= self.s.len() {
                return Err(format!("unclosed <{}>", elem.name));
            }
            if self.starts_with("<!--") {
                self.skip_until("-->")?;
            } else if self.starts_with("<![CDATA[") {
                self.i += 9;
                let off = find(&self.s[self.i..], b"]]>").ok_or("unterminated CDATA")?;
                elem.text.push_str(&fold_newlines(&String::from_utf8_lossy(
                    &self.s[self.i..self.i + off],
                )));
                self.i += off + 3;
            } else if self.starts_with("</") {
                self.i += 2;
                let name = self.name();
                self.skip_ws();
                if name != elem.name || self.s.get(self.i) != Some(&b'>') {
                    return Err(format!("mismatched </{name}> for <{}>", elem.name));
                }
                self.i += 1;
                return Ok(elem);
            } else if self.starts_with("<?") {
                self.skip_until("?>")?;
            } else if self.s[self.i] == b'<' {
                elem.children.push(self.element()?);
            } else {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'<' {
                    self.i += 1;
                }
                elem.text.push_str(&decode_text(&self.s[start..self.i])?);
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn fold_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Character data with entities resolved and newlines folded.
fn decode_text(raw: &[u8]) -> Result<String, String> {
    let s = fold_newlines(&String::from_utf8_lossy(raw));
    if !s.contains('&') {
        return Ok(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').ok_or("unterminated entity")?;
        let ent = &after[..semi];
        let ch = match ent {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "apos" => '\'',
            "quot" => '"',
            _ => {
                let num = ent
                    .strip_prefix('#')
                    .ok_or_else(|| format!("invalid entity &{ent};"))?;
                let n = match num.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16),
                    None => num.parse(),
                }
                .map_err(|_| format!("invalid character entity &{ent};"))?;
                char::from_u32(n).ok_or_else(|| format!("invalid character entity &{ent};"))?
            }
        };
        out.push(ch);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "1" | "t" | "T" | "TRUE" | "true" | "True")
}

fn parse_int(s: &str) -> Result<i64, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("invalid integer {s:?}"))
}

/// The lexer's `<config>` (Go `fastUnmarshalConfig`: the first one anywhere).
pub(crate) fn config(root: &Elem) -> Result<Config, String> {
    let el = root
        .first_descendant("config")
        .ok_or("could not find <config> element")?;
    let mut c = Config::default();
    for child in &el.children {
        let t = &child.text;
        match child.name.as_str() {
            "name" => c.name.clone_from(t),
            "alias" => c.aliases.push(t.clone()),
            "filename" => c.filenames.push(t.clone()),
            "alias_filename" => c.alias_filenames.push(t.clone()),
            "mime_type" => c.mime_types.push(t.clone()),
            "case_insensitive" => c.case_insensitive = parse_bool(t),
            "dot_all" => c.dot_all = parse_bool(t),
            "not_multiline" => c.not_multiline = parse_bool(t),
            "ensure_nl" => c.ensure_nl = parse_bool(t),
            "priority" => {
                c.priority = t
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("invalid priority {t:?}"))?
            }
            _ => {}
        }
    }
    Ok(c)
}

/// The lexer's `<rules>`.
pub(crate) fn rules(root: &Elem) -> Result<Rules, String> {
    let mut rules = HashMap::new();
    let Some(el) = root.child("rules") else {
        return Ok(rules);
    };
    for state in el.children.iter().filter(|c| c.name == "state") {
        let name = state.attr("name").unwrap_or_default().to_string();
        let mut list = Vec::new();
        for rule in state.children.iter().filter(|c| c.name == "rule") {
            list.push(parse_rule(rule)?);
        }
        rules.insert(name, list);
    }
    Ok(rules)
}

fn parse_rule(el: &Elem) -> Result<Rule, String> {
    let mut rule = Rule {
        pattern: el.attr("pattern").unwrap_or_default().to_string(),
        emitter: None,
        mutator: None,
    };
    for child in &el.children {
        if let Some(m) = mutator(child)? {
            if rule.mutator.is_some() {
                return Err("duplicate mutator".into());
            }
            rule.mutator = Some(m);
        } else if let Some(e) = emitter(child)? {
            if rule.emitter.is_some() {
                return Err("duplicate emitter".into());
            }
            rule.emitter = Some(e);
        } else {
            return Err(format!(
                "unknown emitter {:?}: not serialisable",
                child.name
            ));
        }
    }
    Ok(rule)
}

fn mutator(el: &Elem) -> Result<Option<Mutator>, String> {
    Ok(Some(match el.name.as_str() {
        "include" => Mutator::Include(el.attr("state").unwrap_or_default().to_string()),
        "combined" => Mutator::Combined(el.attrs_named("state")),
        "push" => Mutator::Push(el.attrs_named("state")),
        "pop" => Mutator::Pop(el.attr("depth").map(parse_int).transpose()?.unwrap_or(0) as usize),
        "mutators" => {
            let mut list = Vec::new();
            for c in &el.children {
                list.push(
                    mutator(c)?
                        .ok_or_else(|| format!("unknown mutator {:?}: not serialisable", c.name))?,
                );
            }
            Mutator::Multi(list)
        }
        "func" if func_name(el).is_some_and(|n| n.ends_with("haxePreProcMutator")) => {
            Mutator::Func(MutatorFunc::HaxePreProc)
        }
        "rakufindbrackets" => {
            Mutator::RakuFindBrackets(parse_int(el.attr("token").unwrap_or("0"))? as u8)
        }
        "rakureplacerule" => Mutator::RakuReplaceRule(Arc::new(raku_rule(el)?)),
        "rakupoprule" => Mutator::RakuPopRule(Arc::new(raku_rule(el)?)),
        _ => return Ok(None),
    }))
}

fn func_name(el: &Elem) -> Option<&str> {
    el.attr("name")
}

fn optional_emitter(el: &Elem) -> Result<Option<Emitter>, String> {
    if el.name == "nil" {
        return Ok(None);
    }
    emitter(el)?
        .map(Some)
        .ok_or_else(|| format!("unknown emitter {:?}: not serialisable", el.name))
}

fn emitter(el: &Elem) -> Result<Option<Emitter>, String> {
    Ok(Some(match el.name.as_str() {
        "token" => {
            let t = el.attr("type").unwrap_or_default();
            Emitter::Token(
                TokenType::from_name(t)
                    .ok_or_else(|| format!("{t} does not belong to TokenType values"))?,
            )
        }
        "bygroups" => Emitter::ByGroups(
            el.children
                .iter()
                .map(optional_emitter)
                .collect::<Result<_, _>>()?,
        ),
        "usingself" => Emitter::UsingSelf(el.attr("state").unwrap_or_default().to_string()),
        "using" => Emitter::Using(el.attr("lexer").unwrap_or_default().to_string()),
        "usingbygroup" => {
            let int = |name: &str| -> Result<usize, String> {
                Ok(el
                    .child(name)
                    .map(|c| parse_int(&c.text))
                    .transpose()?
                    .unwrap_or(0) as usize)
            };
            let emitters = match el.child("emitters") {
                Some(list) => list
                    .children
                    .iter()
                    .map(optional_emitter)
                    .collect::<Result<_, _>>()?,
                None => Vec::new(),
            };
            Emitter::UsingByGroup {
                sublexer_name_group: int("sublexer_name_group")?,
                code_group: int("code_group")?,
                emitters,
            }
        }
        "bygroupnames" => {
            let mut groups = Vec::new();
            for g in el.children.iter().filter(|c| c.name == "group") {
                let inner = match g.children.first() {
                    Some(e) => optional_emitter(e)?,
                    None => None,
                };
                groups.push((g.attr("name").unwrap_or_default().to_string(), inner));
            }
            Emitter::ByGroupNames(groups)
        }
        "rakucolonpair" => {
            let t = el.attr("token").unwrap_or_default();
            Emitter::RakuColonPair(
                TokenType::from_name(t).ok_or_else(|| format!("unknown token type {t}"))?,
            )
        }
        "func" => {
            let name = func_name(el).unwrap_or_default();
            let f = [
                ("UsingLexer.func1", EmitterFunc::GoTemplateString),
                ("httpHeaderBlock", EmitterFunc::HttpHeaderBlock),
                (
                    "httpContinuousHeaderBlock",
                    EmitterFunc::HttpContinuousHeaderBlock,
                ),
                ("httpContentBlock", EmitterFunc::HttpContentBlock),
                ("rstCodeBlock", EmitterFunc::RstCodeBlock),
                ("lexers.quote", EmitterFunc::RakuQuote),
                ("podConfig", EmitterFunc::RakuPodConfig),
                ("podCode", EmitterFunc::RakuPodCode),
            ]
            .into_iter()
            .find(|(suffix, _)| name.ends_with(suffix))
            .map(|(_, f)| f)
            .ok_or_else(|| format!("unknown emitter func {name}"))?;
            Emitter::Func(f)
        }
        _ => return Ok(None),
    }))
}

fn raku_rule(el: &Elem) -> Result<RakuRuleConfig, String> {
    let get = |k: &str| el.attr(k).unwrap_or_default();
    let section = |k: &str| el.child(k).and_then(|c| c.children.first());
    Ok(RakuRuleConfig {
        delimiter: get("delimiter").chars().collect(),
        pattern: get("pattern").to_string(),
        token_type: section("emitter")
            .map(optional_emitter)
            .transpose()?
            .flatten(),
        number_of_delimiter_chars: parse_int(get("ndelim"))? as usize,
        mutator: section("mutator").map(mutator).transpose()?.flatten(),
        append_mutator: section("append").map(mutator).transpose()?.flatten(),
        rule_position: if parse_int(get("position"))? == 0 {
            RulePosition::Top
        } else {
            RulePosition::Bottom
        },
        state_name: get("state").to_string(),
        pop: parse_bool(get("pop")),
        pop_state: parse_bool(get("popstate")),
        push_state: parse_bool(get("pushstate")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attributes_entities_and_comments() {
        let doc = "<?xml version=\"1.0\"?>\r\n<lexer>\r\n  <!-- c -->\r\n  <config><name>A &amp; B</name><priority>0.1</priority> <!-- x --></config>\r\n  <rules><state name=\"root\"><rule pattern=\"&#34;a\r\nb&lt;\"><push state=\"x\" state=\"y\"/></rule></state></rules>\r\n</lexer>";
        let root = parse(doc).unwrap_or_else(|e| panic!("{e}"));
        let c = config(&root).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.name, "A & B");
        assert!((c.priority - 0.1).abs() < f32::EPSILON);
        let rules = rules(&root).unwrap_or_else(|e| panic!("{e}"));
        let r = &rules["root"][0];
        assert_eq!(r.pattern, "\"a\nb<");
        assert!(
            matches!(&r.mutator, Some(Mutator::Push(s)) if s == &["x".to_string(), "y".to_string()])
        );
    }
}
