//! Compiled grammar rules, a port of vscode-textmate's `rule.ts`.

use std::sync::{Arc, Mutex};

use crate::onig_lib::{CaptureIndex, OnigError, OnigScanner, OnigString};
use crate::utils::{escape_reg_exp_characters, has_captures, replace_captures};

pub(crate) type RuleId = u32;

/// Scanner result id meaning "the `end` pattern of the current rule matched".
pub(crate) const END_RULE_ID: i64 = -1;
/// Scanner result id meaning "the `while` pattern matched".
pub(crate) const WHILE_RULE_ID: i64 = -2;

const NEVER: &str = "\u{FFFF}";

fn has_back_references(source: &str) -> bool {
    let b = source.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| b[i] == b'\\' && b[i + 1].is_ascii_digit())
}

#[derive(Clone, Debug)]
pub(crate) struct RegExpSource {
    pub source: String,
    pub rule_id: i64,
    pub has_anchor: bool,
    pub has_back_references: bool,
    /// Variants with `\A` / `\G` disabled: [A0_G0, A0_G1, A1_G0, A1_G1].
    anchor_cache: Option<[String; 4]>,
}

impl RegExpSource {
    pub fn new(source: &str, rule_id: i64) -> RegExpSource {
        let b = source.as_bytes();
        let mut out = String::new();
        let mut last = 0;
        let mut has_anchor = false;
        let mut pos = 0;
        while pos < b.len() {
            if b[pos] == b'\\' && pos + 1 < b.len() {
                match b[pos + 1] {
                    b'z' => {
                        out.push_str(&source[last..pos]);
                        out.push_str("$(?!\\n)(?<!\\n)");
                        last = pos + 2;
                    }
                    b'A' | b'G' => has_anchor = true,
                    _ => {}
                }
                pos += 1;
            }
            pos += 1;
        }
        let source = if last == 0 {
            source.to_string()
        } else {
            out.push_str(&source[last..]);
            out
        };
        let mut r = RegExpSource {
            has_back_references: has_back_references(&source),
            source,
            rule_id,
            has_anchor,
            anchor_cache: None,
        };
        if r.has_anchor {
            r.anchor_cache = Some(r.build_anchor_cache());
        }
        r
    }

    pub fn set_source(&mut self, new_source: &str) {
        if self.source == new_source {
            return;
        }
        self.source = new_source.to_string();
        if self.has_anchor {
            self.anchor_cache = Some(self.build_anchor_cache());
        }
    }

    pub fn resolve_back_references(&self, line_text: &str, captures: &[CaptureIndex]) -> String {
        let values: Vec<&str> = captures
            .iter()
            .map(|c| line_text.get(c.start..c.end).unwrap_or(""))
            .collect();
        let b = self.source.as_bytes();
        let mut out = String::with_capacity(self.source.len());
        let mut last = 0;
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' {
                let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if digits > 0 {
                    out.push_str(&self.source[last..i]);
                    let idx: usize = self.source[i + 1..i + 1 + digits]
                        .parse()
                        .unwrap_or(usize::MAX);
                    out.push_str(&escape_reg_exp_characters(
                        values.get(idx).copied().unwrap_or(""),
                    ));
                    i += 1 + digits;
                    last = i;
                    continue;
                }
            }
            i += 1;
        }
        out.push_str(&self.source[last..]);
        out
    }

    fn build_anchor_cache(&self) -> [String; 4] {
        let mut r: [String; 4] = Default::default();
        let mut chars = self.source.chars().peekable();
        while let Some(ch) = chars.next() {
            for s in &mut r {
                s.push(ch);
            }
            if ch == '\\'
                && let Some(&next) = chars.peek()
            {
                chars.next();
                let (a0g0, a0g1, a1g0, a1g1) = match next {
                    'A' => (NEVER, NEVER, "A", "A"),
                    'G' => (NEVER, "G", NEVER, "G"),
                    _ => ("", "", "", ""),
                };
                if a0g0.is_empty() {
                    for s in &mut r {
                        s.push(next);
                    }
                } else {
                    r[0].push_str(a0g0);
                    r[1].push_str(a0g1);
                    r[2].push_str(a1g0);
                    r[3].push_str(a1g1);
                }
            }
        }
        r
    }

    pub fn resolve_anchors(&self, allow_a: bool, allow_g: bool) -> &str {
        match &self.anchor_cache {
            Some(c) if self.has_anchor => &c[anchor_index(allow_a, allow_g)],
            _ => &self.source,
        }
    }
}

fn anchor_index(allow_a: bool, allow_g: bool) -> usize {
    (usize::from(allow_a) << 1) | usize::from(allow_g)
}

pub(crate) struct CompiledRule {
    scanner: OnigScanner,
    rules: Vec<i64>,
}

pub(crate) struct FindResult {
    pub rule_id: i64,
    pub capture_indices: Vec<CaptureIndex>,
}

impl CompiledRule {
    fn new(sources: &[&str], rules: Vec<i64>) -> Result<CompiledRule, OnigError> {
        Ok(CompiledRule {
            scanner: OnigScanner::new(sources)?,
            rules,
        })
    }

    pub fn find_next_match(&self, s: &OnigString, start: usize) -> Option<FindResult> {
        let m = self.scanner.find_next_match(s, start)?;
        Some(FindResult {
            rule_id: self.rules[m.index],
            capture_indices: m.capture_indices,
        })
    }
}

#[derive(Default)]
pub(crate) struct RegExpSourceList {
    items: Vec<RegExpSource>,
    has_anchors: bool,
    cached: Option<Arc<CompiledRule>>,
    anchor_cache: [Option<Arc<CompiledRule>>; 4],
}

impl RegExpSourceList {
    pub fn push(&mut self, item: RegExpSource) {
        self.has_anchors |= item.has_anchor;
        self.items.push(item);
    }

    pub fn unshift(&mut self, item: RegExpSource) {
        self.has_anchors |= item.has_anchor;
        self.items.insert(0, item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn set_source(&mut self, index: usize, new_source: &str) {
        if self.items[index].source != new_source {
            self.cached = None;
            self.anchor_cache = Default::default();
            self.items[index].set_source(new_source);
        }
    }

    pub fn compile(&mut self) -> Result<Arc<CompiledRule>, OnigError> {
        if self.cached.is_none() {
            let sources: Vec<&str> = self.items.iter().map(|e| e.source.as_str()).collect();
            let rules = self.items.iter().map(|e| e.rule_id).collect();
            self.cached = Some(Arc::new(CompiledRule::new(&sources, rules)?));
        }
        Ok(self.cached.clone().expect("compiled above"))
    }

    pub fn compile_ag(
        &mut self,
        allow_a: bool,
        allow_g: bool,
    ) -> Result<Arc<CompiledRule>, OnigError> {
        if !self.has_anchors {
            return self.compile();
        }
        let i = anchor_index(allow_a, allow_g);
        if self.anchor_cache[i].is_none() {
            let sources: Vec<&str> = self
                .items
                .iter()
                .map(|e| e.resolve_anchors(allow_a, allow_g))
                .collect();
            let rules = self.items.iter().map(|e| e.rule_id).collect();
            self.anchor_cache[i] = Some(Arc::new(CompiledRule::new(&sources, rules)?));
        }
        Ok(self.anchor_cache[i].clone().expect("compiled above"))
    }
}

pub(crate) enum RuleKind {
    Capture {
        retokenize_captured_with_rule_id: RuleId,
    },
    Match {
        match_: RegExpSource,
        captures: Vec<Option<RuleId>>,
        cached: Mutex<Option<RegExpSourceList>>,
    },
    IncludeOnly {
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
        cached: Mutex<Option<RegExpSourceList>>,
    },
    BeginEnd {
        begin: RegExpSource,
        begin_captures: Vec<Option<RuleId>>,
        end: RegExpSource,
        end_captures: Vec<Option<RuleId>>,
        apply_end_pattern_last: bool,
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
        cached: Mutex<Option<RegExpSourceList>>,
    },
    BeginWhile {
        begin: RegExpSource,
        begin_captures: Vec<Option<RuleId>>,
        while_: RegExpSource,
        while_captures: Vec<Option<RuleId>>,
        patterns: Vec<RuleId>,
        has_missing_patterns: bool,
        cached: Mutex<Option<RegExpSourceList>>,
        cached_while: Mutex<Option<RegExpSourceList>>,
    },
}

pub(crate) struct Rule {
    name: Option<String>,
    name_is_capturing: bool,
    content_name: Option<String>,
    content_name_is_capturing: bool,
    pub kind: RuleKind,
}

/// Rule lookup for pattern collection (the grammar's rule registry).
pub(crate) trait RuleRegistry {
    fn rule(&self, id: RuleId) -> Option<Arc<Rule>>;
}

impl Rule {
    pub fn new(name: Option<String>, content_name: Option<String>, kind: RuleKind) -> Rule {
        let name = name.filter(|n| !n.is_empty());
        let content_name = content_name.filter(|n| !n.is_empty());
        Rule {
            name_is_capturing: has_captures(name.as_deref()),
            content_name_is_capturing: has_captures(content_name.as_deref()),
            name,
            content_name,
            kind,
        }
    }

    pub fn get_name(
        &self,
        line_text: Option<&str>,
        captures: Option<&[CaptureIndex]>,
    ) -> Option<String> {
        match (&self.name, line_text, captures) {
            (Some(name), Some(text), Some(caps)) if self.name_is_capturing => {
                Some(replace_captures(name, text, caps))
            }
            _ => self.name.clone(),
        }
    }

    pub fn get_content_name(&self, line_text: &str, captures: &[CaptureIndex]) -> Option<String> {
        match &self.content_name {
            Some(name) if self.content_name_is_capturing => {
                Some(replace_captures(name, line_text, captures))
            }
            other => other.clone(),
        }
    }

    pub fn patterns_missing(&self) -> Option<(&[RuleId], bool)> {
        match &self.kind {
            RuleKind::IncludeOnly {
                patterns,
                has_missing_patterns,
                ..
            }
            | RuleKind::BeginEnd {
                patterns,
                has_missing_patterns,
                ..
            }
            | RuleKind::BeginWhile {
                patterns,
                has_missing_patterns,
                ..
            } => Some((patterns, *has_missing_patterns)),
            _ => None,
        }
    }

    pub fn collect_patterns(&self, reg: &dyn RuleRegistry, out: &mut RegExpSourceList) {
        match &self.kind {
            // A capture rule is never part of a pattern list.
            RuleKind::Capture { .. } => {}
            RuleKind::Match { match_, .. } => out.push(match_.clone()),
            RuleKind::IncludeOnly { patterns, .. } => {
                for p in patterns {
                    if let Some(r) = reg.rule(*p) {
                        r.collect_patterns(reg, out);
                    }
                }
            }
            RuleKind::BeginEnd { begin, .. } | RuleKind::BeginWhile { begin, .. } => {
                out.push(begin.clone())
            }
        }
    }

    fn collect_list(reg: &dyn RuleRegistry, patterns: &[RuleId]) -> RegExpSourceList {
        let mut list = RegExpSourceList::default();
        for p in patterns {
            if let Some(r) = reg.rule(*p) {
                r.collect_patterns(reg, &mut list);
            }
        }
        list
    }

    /// `compileAG`: the scanner for this rule's patterns with `\A` / `\G`
    /// enabled or not. `end_regex_source` is the stack's resolved `end`.
    pub fn compile_ag(
        &self,
        reg: &dyn RuleRegistry,
        end_regex_source: Option<&str>,
        allow_a: bool,
        allow_g: bool,
    ) -> Result<Arc<CompiledRule>, OnigError> {
        match &self.kind {
            RuleKind::Capture { .. } => {
                let mut empty = RegExpSourceList::default();
                empty.compile()
            }
            RuleKind::Match { match_, cached, .. } => {
                let mut c = cached.lock().unwrap();
                let list = c.get_or_insert_with(|| {
                    let mut l = RegExpSourceList::default();
                    l.push(match_.clone());
                    l
                });
                list.compile_ag(allow_a, allow_g)
            }
            RuleKind::IncludeOnly {
                patterns, cached, ..
            }
            | RuleKind::BeginWhile {
                patterns, cached, ..
            } => {
                let mut c = cached.lock().unwrap();
                c.get_or_insert_with(|| Self::collect_list(reg, patterns))
                    .compile_ag(allow_a, allow_g)
            }
            RuleKind::BeginEnd {
                end,
                apply_end_pattern_last,
                patterns,
                cached,
                ..
            } => {
                let mut c = cached.lock().unwrap();
                let list = c.get_or_insert_with(|| {
                    let mut list = Self::collect_list(reg, patterns);
                    if *apply_end_pattern_last {
                        list.push(end.clone());
                    } else {
                        list.unshift(end.clone());
                    }
                    list
                });
                if end.has_back_references
                    && let Some(src) = end_regex_source
                {
                    let i = if *apply_end_pattern_last {
                        list.len() - 1
                    } else {
                        0
                    };
                    list.set_source(i, src);
                }
                list.compile_ag(allow_a, allow_g)
            }
        }
    }

    /// The `while` scanner of a begin/while rule.
    pub fn compile_while_ag(
        &self,
        end_regex_source: Option<&str>,
        allow_a: bool,
        allow_g: bool,
    ) -> Result<Arc<CompiledRule>, OnigError> {
        let RuleKind::BeginWhile {
            while_,
            cached_while,
            ..
        } = &self.kind
        else {
            let mut empty = RegExpSourceList::default();
            return empty.compile();
        };
        let mut c = cached_while.lock().unwrap();
        let list = c.get_or_insert_with(|| {
            let mut l = RegExpSourceList::default();
            l.push(while_.clone());
            l
        });
        if while_.has_back_references {
            list.set_source(
                0,
                end_regex_source.filter(|s| !s.is_empty()).unwrap_or(NEVER),
            );
        }
        list.compile_ag(allow_a, allow_g)
    }

    pub fn end_with_resolved_back_references(
        &self,
        line_text: &str,
        captures: &[CaptureIndex],
    ) -> Option<String> {
        match &self.kind {
            RuleKind::BeginEnd { end, .. } => {
                Some(end.resolve_back_references(line_text, captures))
            }
            RuleKind::BeginWhile { while_, .. } => {
                Some(while_.resolve_back_references(line_text, captures))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_backslash_z() {
        let r = RegExpSource::new("a\\z", 1);
        assert_eq!(r.source, "a$(?!\\n)(?<!\\n)");
        assert!(!r.has_anchor);
    }

    #[test]
    fn builds_anchor_variants() {
        let r = RegExpSource::new("\\G\\Ax\\\\G", 1);
        assert!(r.has_anchor);
        assert_eq!(
            r.resolve_anchors(false, false),
            "\\\u{FFFF}\\\u{FFFF}x\\\\G"
        );
        assert_eq!(r.resolve_anchors(false, true), "\\G\\\u{FFFF}x\\\\G");
        assert_eq!(r.resolve_anchors(true, false), "\\\u{FFFF}\\Ax\\\\G");
        assert_eq!(r.resolve_anchors(true, true), "\\G\\Ax\\\\G");
    }

    #[test]
    fn resolves_back_references_escaped() {
        let r = RegExpSource::new("\\1|\\2end", -1);
        assert!(r.has_back_references);
        let caps = [
            CaptureIndex { start: 0, end: 4 },
            CaptureIndex { start: 0, end: 2 },
        ];
        assert_eq!(r.resolve_back_references("a.b ", &caps), "a\\.|end");
    }
}
