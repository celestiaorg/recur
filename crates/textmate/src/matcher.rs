//! Injection / token-type selector matcher (`matcher.ts`), with the grammar's
//! `nameMatcher` as the leaf test.

#[derive(Debug, Clone)]
enum Expr {
    Names(Vec<String>),
    Not(Option<Box<Expr>>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl Expr {
    fn eval(&self, scopes: &[String]) -> bool {
        match self {
            Expr::Names(ids) => name_matcher(ids, scopes),
            Expr::Not(inner) => inner.as_ref().is_some_and(|e| !e.eval(scopes)),
            Expr::And(all) => all.iter().all(|e| e.eval(scopes)),
            Expr::Or(any) => any.iter().any(|e| e.eval(scopes)),
        }
    }
}

/// A compiled selector alternative. `priority` is -1 for `L:`, 1 for `R:`.
#[derive(Debug, Clone)]
pub struct Matcher {
    expr: Expr,
    pub priority: i8,
}

impl Matcher {
    pub fn matches(&self, scopes: &[String]) -> bool {
        self.expr.eval(scopes)
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `/([LR]:|[\w\.:][\w\.:\-]*|[\,\|\-\(\)])/g`
fn tokenize(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c == 'L' || c == 'R') && chars.get(i + 1) == Some(&':') {
            out.push(format!("{c}:"));
            i += 2;
        } else if is_word(c) || c == '.' || c == ':' {
            let start = i;
            i += 1;
            while i < chars.len() && (is_word(chars[i]) || matches!(chars[i], '.' | ':' | '-')) {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else if matches!(c, ',' | '|' | '-' | '(' | ')') {
            out.push(c.to_string());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn is_identifier(token: Option<&str>) -> bool {
    token.is_some_and(|t| t.chars().any(|c| is_word(c) || c == '.' || c == ':'))
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
    token: Option<String>,
}

impl Parser {
    fn next(&mut self) {
        self.token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
    }

    fn tok(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn parse_operand(&mut self) -> Option<Expr> {
        if self.tok() == Some("-") {
            self.next();
            let negated = self.parse_operand();
            return Some(Expr::Not(negated.map(Box::new)));
        }
        if self.tok() == Some("(") {
            self.next();
            let inner = self.parse_inner_expression();
            if self.tok() == Some(")") {
                self.next();
            }
            return Some(inner);
        }
        if is_identifier(self.tok()) {
            let mut ids = Vec::new();
            while is_identifier(self.tok()) {
                ids.push(self.token.clone().unwrap_or_default());
                self.next();
            }
            return Some(Expr::Names(ids));
        }
        None
    }

    fn parse_conjunction(&mut self) -> Expr {
        let mut all = Vec::new();
        while let Some(m) = self.parse_operand() {
            all.push(m);
        }
        Expr::And(all)
    }

    fn parse_inner_expression(&mut self) -> Expr {
        let mut any = Vec::new();
        loop {
            any.push(self.parse_conjunction());
            if matches!(self.tok(), Some("|" | ",")) {
                while matches!(self.tok(), Some("|" | ",")) {
                    self.next();
                }
            } else {
                break;
            }
        }
        Expr::Or(any)
    }
}

pub fn create_matchers(selector: &str) -> Vec<Matcher> {
    let mut p = Parser {
        tokens: tokenize(selector),
        pos: 0,
        token: None,
    };
    p.next();
    let mut out = Vec::new();
    while p.token.is_some() {
        let mut priority = 0;
        if let Some(t) = p.tok()
            && t.len() == 2
            && t.ends_with(':')
        {
            priority = match t {
                "R:" => 1,
                "L:" => -1,
                _ => 0,
            };
            p.next();
        }
        let expr = p.parse_conjunction();
        out.push(Matcher { expr, priority });
        if p.tok() != Some(",") {
            break;
        }
        p.next();
    }
    out
}

pub(crate) fn scopes_are_matching(this_scope_name: &str, scope_name: &str) -> bool {
    if this_scope_name.is_empty() {
        return false;
    }
    if this_scope_name == scope_name {
        return true;
    }
    let len = scope_name.len();
    this_scope_name.len() > len
        && this_scope_name.starts_with(scope_name)
        && this_scope_name.as_bytes()[len] == b'.'
}

pub(crate) fn name_matcher(identifiers: &[String], scopes: &[String]) -> bool {
    if scopes.len() < identifiers.len() {
        return false;
    }
    let mut last = 0;
    identifiers.iter().all(|id| {
        for (i, s) in scopes.iter().enumerate().skip(last) {
            if scopes_are_matching(s, id) {
                last = i + 1;
                return true;
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn check(selector: &str, input: &[&str], want: bool) {
        let input = s(input);
        let got = create_matchers(selector).iter().any(|m| m.matches(&input));
        assert_eq!(got, want, "{selector} vs {input:?}");
    }

    // Cases from vscode-textmate src/tests/matcher.test.ts.
    #[test]
    fn upstream_matcher_cases() {
        check("foo", &["foo"], true);
        check("foo", &["bar"], false);
        check("- foo", &["foo"], false);
        check("- foo", &["bar"], true);
        check("- - foo", &["bar"], false);
        check("bar foo", &["foo"], false);
        check("bar foo", &["bar"], false);
        check("bar foo", &["bar", "foo"], true);
        check("bar - foo", &["bar"], true);
        check("bar - foo", &["foo", "bar"], false);
        check("bar - foo", &["foo"], false);
        check("bar, foo", &["foo"], true);
        check("bar, foo", &["bar"], true);
        check("bar, foo", &["bar", "foo"], true);
        check("bar, -foo", &["bar", "foo"], true);
        check("bar, -foo", &["yo"], true);
        check("bar, -foo", &["foo"], false);
        check("(foo)", &["foo"], true);
        check("(foo - bar)", &["foo"], true);
        check("(foo - bar)", &["foo", "bar"], false);
        check("foo bar - (yo man)", &["foo", "bar"], true);
        check("foo bar - (yo man)", &["foo", "bar", "yo"], true);
        check("foo bar - (yo man)", &["foo", "bar", "yo", "man"], false);
        check("foo bar - (yo | man)", &["foo", "bar", "yo", "man"], false);
        check("foo bar - (yo | man)", &["foo", "bar", "yo"], false);
        check(
            "R:text.html - (comment.block, text.html source)",
            &["text.html", "bar", "source"],
            false,
        );
        check(
            "text.html.php - (meta.embedded | meta.tag), L:text.html.php meta.tag, L:source.js.embedded.html",
            &["text.html.php", "bar", "source.js"],
            true,
        );
    }

    #[test]
    fn priorities_and_alternatives() {
        let ms = create_matchers("L:source.ts - comment, R:text.html");
        assert_eq!(ms.iter().map(|m| m.priority).collect::<Vec<_>>(), [-1, 1]);
    }
}
