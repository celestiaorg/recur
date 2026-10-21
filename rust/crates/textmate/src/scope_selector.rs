//! TextMate scope selectors for `[[symbol]]` rules (`tm-scope-selector.ts`).
//!
//! Subset: comma OR, space descendant, `>` child, ` - ` exclusion. An atom
//! matches a stack entry by dotted prefix.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeAtom {
    pub prefix: String,
    /// Follows `>`: must be the next stack entry.
    pub child: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeConjunction {
    pub atoms: Vec<ScopeAtom>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeAlternative {
    pub include: ScopeConjunction,
    pub exclude: Vec<ScopeConjunction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledSelector {
    pub alts: Vec<ScopeAlternative>,
}

pub fn scope_prefix_match(want: &str, have: &str) -> bool {
    have == want || (have.starts_with(want) && have.as_bytes().get(want.len()) == Some(&b'.'))
}

fn parse_conjunction(src: &str) -> ScopeConjunction {
    let mut atoms = Vec::new();
    let mut pending_child = false;
    for part in src.split_whitespace() {
        if part == ">" {
            pending_child = true;
            continue;
        }
        let prefix = part.trim_start_matches('>');
        if prefix.is_empty() {
            continue;
        }
        let child = pending_child || part.starts_with('>');
        pending_child = false;
        let child = child && !atoms.is_empty();
        atoms.push(ScopeAtom {
            prefix: prefix.to_string(),
            child,
        });
    }
    ScopeConjunction { atoms }
}

fn parse_alternative(src: &str) -> ScopeAlternative {
    let mut chunks = src.split(" - ");
    let include = parse_conjunction(chunks.next().unwrap_or(""));
    ScopeAlternative {
        include,
        exclude: chunks.map(parse_conjunction).collect(),
    }
}

pub fn compile_selector(src: &str) -> CompiledSelector {
    CompiledSelector {
        alts: src
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(parse_alternative)
            .collect(),
    }
}

fn match_conjunction(conj: &ScopeConjunction, scopes: &[String]) -> Option<usize> {
    if conj.atoms.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut score = conj.atoms.len() * 1000;
    for atom in &conj.atoms {
        let found = if atom.child {
            (i < scopes.len() && scope_prefix_match(&atom.prefix, &scopes[i])).then_some(i)
        } else {
            (i..scopes.len()).find(|&j| scope_prefix_match(&atom.prefix, &scopes[j]))
        }?;
        score += atom.prefix.split('.').filter(|s| !s.is_empty()).count() * 10 + found;
        i = found + 1;
    }
    Some(score)
}

/// Highest matching alternative's score; 0 when nothing matches.
pub fn selector_score(sel: &CompiledSelector, scopes: &[String]) -> usize {
    let mut best = 0;
    for alt in &sel.alts {
        let Some(score) = match_conjunction(&alt.include, scopes) else {
            continue;
        };
        let excluded = alt
            .exclude
            .iter()
            .any(|ex| match_conjunction(ex, scopes).is_some());
        if !excluded && score > best {
            best = score;
        }
    }
    best
}

pub fn selector_matches(sel: &CompiledSelector, scopes: &[String]) -> bool {
    selector_score(sel, scopes) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn m(sel: &str, scopes: &[&str]) -> bool {
        selector_matches(&compile_selector(sel), &s(scopes))
    }

    #[test]
    fn prefix_match() {
        assert!(scope_prefix_match(
            "entity.name.function",
            "entity.name.function"
        ));
        assert!(scope_prefix_match(
            "entity.name.function",
            "entity.name.function.go"
        ));
        assert!(!scope_prefix_match(
            "entity.name.function",
            "entity.name.type.go"
        ));
        assert!(scope_prefix_match("string", "string.quoted.double.html"));
    }

    #[test]
    fn selector_matching() {
        let go_fn = ["source.go", "meta.function.go", "entity.name.function.go"];
        let go_support = ["source.go", "entity.name.function.support.go"];
        let html_id = [
            "text.html.basic",
            "meta.tag.inline.any.html",
            "meta.attribute.id.html",
            "string.quoted.double.html",
        ];
        assert!(m("entity.name.function", &go_fn));
        assert!(!m("entity.name.type", &go_fn));
        assert!(m("source.go entity.name.function", &go_fn));
        assert!(!m("source.python entity.name.function", &go_fn));
        let alt = "source.css meta.selector, source.css meta.at-rule.media";
        assert!(m(alt, &["source.css", "meta.selector.css"]));
        assert!(m(alt, &["source.css", "meta.at-rule.media.css"]));
        assert!(!m(alt, &["source.go", "meta.selector.css"]));
        assert!(m(
            "entity.name.function - entity.name.function.support",
            &go_fn
        ));
        assert!(!m(
            "entity.name.function - entity.name.function.support",
            &go_support
        ));
        assert!(m("meta.attribute.id.html > string", &html_id));
        assert!(!m(
            "meta.attribute.id.html > string",
            &[
                "text.html.basic",
                "string.quoted.double.html",
                "meta.attribute.id.html"
            ]
        ));
        let generic = compile_selector("entity.name.function");
        let support = compile_selector("entity.name.function.support");
        assert!(
            selector_score(&support, &s(&go_support)) > selector_score(&generic, &s(&go_support))
        );
    }
}
