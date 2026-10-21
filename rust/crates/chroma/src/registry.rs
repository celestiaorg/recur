//! The lexer registry (chroma `lexers.GlobalLexerRegistry`): the embedded
//! XML lexers in file order, then the lexers chroma builds in Go, registered
//! in the same order so filename matches break ties identically.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use include_dir::{Dir, include_dir};

use super::glob;
use super::lexer::{Lexer, RegexLexer};
use super::token::TokenType;
use crate::text::go_lower;

static DATA: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/data");

/// Backup and template suffixes a filename glob also matches through.
const IGNORED_SUFFIXES: [&str; 13] = [
    "~",
    ".bak",
    ".old",
    ".orig",
    ".dpkg-dist",
    ".dpkg-old",
    ".ucf-dist",
    ".ucf-new",
    ".ucf-old",
    ".rpmnew",
    ".rpmorig",
    ".rpmsave",
    ".in",
];

#[derive(Default)]
pub(crate) struct Registry {
    pub lexers: Vec<Arc<Lexer>>,
    by_name: HashMap<String, Arc<Lexer>>,
    by_alias: HashMap<String, Arc<Lexer>>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(build);

pub(crate) fn registry() -> &'static Registry {
    &REGISTRY
}

/// A lexer by name, alias, or file extension (chroma `lexers.Get`).
pub(crate) fn get(name: &str) -> Option<Arc<Lexer>> {
    registry().get(name)
}

pub(crate) fn data(path: &str) -> &'static str {
    DATA.get_file(path)
        .and_then(|f| f.contents_utf8())
        .unwrap_or_default()
}

fn regex_lexer(path: &str) -> Arc<Lexer> {
    let lexer =
        RegexLexer::from_xml(data(path)).unwrap_or_else(|e| panic!("embedded lexer {path}: {e}"));
    Arc::new(Lexer::Regex(Arc::new(lexer)))
}

/// A TypeRemappingLexer table: `word<TAB>TokenType` lines remapping
/// `NameVariable`.
fn remapping(xml: &str, table: &str) -> Arc<Lexer> {
    let words: HashMap<String, TokenType> = data(table)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(w, t)| TokenType::from_name(t).map(|t| (w.to_string(), t)))
        .collect();
    let lut = HashMap::from([(TokenType::NAME_VARIABLE, words)]);
    Arc::new(Lexer::Remapping {
        inner: regex_lexer(xml),
        lut,
    })
}

fn build() -> Registry {
    let mut reg = Registry::default();
    let mut files: Vec<&str> = DATA
        .get_dir("lexers")
        .map(|d| {
            d.files()
                .filter_map(|f| f.path().to_str())
                .filter(|p| p.ends_with(".xml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort_unstable();
    for path in files {
        reg.register(regex_lexer(path));
    }

    let html = regex_lexer("lexers/html.xml");
    let delegating = |root: &Arc<Lexer>, language: &str| {
        Arc::new(Lexer::Delegating {
            root: root.clone(),
            language: regex_lexer(language),
        })
    };
    reg.register(remapping("lexers/common_lisp.xml", "remap_common_lisp.tsv"));
    reg.register(remapping("lexers/emacslisp.xml", "remap_emacslisp.tsv"));
    reg.register(delegating(&html, "generated/erb_language.xml"));
    for path in [
        "generated/caddyfile.xml",
        "generated/caddyfile_directives.xml",
        "generated/gemtext.xml",
        "generated/genshi_text.xml",
        "generated/genshi_html.xml",
        "generated/genshi.xml",
    ] {
        reg.register(regex_lexer(path));
    }
    reg.register(delegating(&html, "generated/go_html_template_language.xml"));
    reg.register(regex_lexer("generated/go_text_template.xml"));
    reg.register(regex_lexer("generated/go.xml"));
    reg.register(regex_lexer("generated/haxe.xml"));
    reg.register(Arc::new(Lexer::Http(regex_lexer("generated/http.xml"))));
    reg.register(Arc::new(Lexer::Markdown(regex_lexer(
        "generated/markdown.xml",
    ))));
    reg.register(regex_lexer("generated/markless.xml"));
    reg.register(delegating(&html, "generated/phtml_language.xml"));
    reg.register(regex_lexer("generated/raku.xml"));
    reg.register(regex_lexer("generated/restructuredtext.xml"));
    reg.register(delegating(&html, "generated/svelte_language.xml"));
    reg.register(delegating(
        &regex_lexer("lexers/yaml.xml"),
        "generated/yaml_jinja_language.xml",
    ));
    reg
}

fn priority(l: &Lexer) -> f32 {
    match l.config().priority {
        0.0 => 1.0,
        p => p,
    }
}

/// Sorts by descending priority, keeping registration order for ties (Go's
/// `sort.Sort` is an insertion sort at these sizes).
fn prioritise(mut lexers: Vec<Arc<Lexer>>) -> Option<Arc<Lexer>> {
    lexers.sort_by(|a, b| {
        priority(b)
            .partial_cmp(&priority(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    lexers.into_iter().next()
}

impl Registry {
    /// Registers `lexer`, replacing a registered lexer of the same name in place.
    fn register(&mut self, lexer: Arc<Lexer>) {
        let config = lexer.config();
        self.by_name.insert(config.name.clone(), lexer.clone());
        self.by_name.insert(go_lower(&config.name), lexer.clone());
        for alias in &config.aliases {
            self.by_alias.insert(alias.clone(), lexer.clone());
            self.by_alias.insert(go_lower(alias), lexer.clone());
        }
        match self
            .lexers
            .iter()
            .position(|l| l.config().name == config.name)
        {
            Some(i) => self.lexers[i] = lexer,
            None => self.lexers.push(lexer),
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<Arc<Lexer>> {
        let lower = go_lower(name);
        if let Some(l) = self
            .by_name
            .get(name)
            .or_else(|| self.by_alias.get(name))
            .or_else(|| self.by_name.get(&lower))
            .or_else(|| self.by_alias.get(&lower))
        {
            return Some(l.clone());
        }
        let candidates: Vec<Arc<Lexer>> = [
            self.match_filename(&format!("filename.{name}")),
            self.match_filename(name),
        ]
        .into_iter()
        .flatten()
        .collect();
        prioritise(candidates)
    }

    /// The best lexer for a filename's base name (chroma `Match`).
    pub(crate) fn match_filename(&self, filename: &str) -> Option<Arc<Lexer>> {
        let base = crate::text::base(filename);
        let primary = self.matching(&base, |l| &l.config().filenames);
        if primary.is_some() {
            return primary;
        }
        self.matching(&base, |l| &l.config().alias_filenames)
    }

    fn matching(
        &self,
        filename: &str,
        globs: impl Fn(&Lexer) -> &Vec<String>,
    ) -> Option<Arc<Lexer>> {
        let mut matched = Vec::new();
        for lexer in &self.lexers {
            for g in globs(lexer) {
                if glob::matches(g, filename).unwrap_or(false)
                    || IGNORED_SUFFIXES
                        .iter()
                        .any(|suf| glob::matches(&format!("{g}{suf}"), filename).unwrap_or(false))
                {
                    matched.push(lexer.clone());
                }
            }
        }
        prioritise(matched)
    }

    /// The best lexer for a MIME type (chroma `MatchMimeType`).
    pub(crate) fn match_mime_type(&self, mime_type: &str) -> Option<Arc<Lexer>> {
        let matched: Vec<Arc<Lexer>> = self
            .lexers
            .iter()
            .flat_map(|l| {
                l.config()
                    .mime_types
                    .iter()
                    .filter(|m| *m == mime_type)
                    .map(move |_| l.clone())
            })
            .collect();
        prioritise(matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_chroma_order_and_names() {
        let reg = registry();
        assert_eq!(reg.lexers.len(), 297);
        assert_eq!(reg.lexers[49].config().name, "Common Lisp");
        assert_eq!(reg.lexers[72].config().name, "ERB");
        assert_eq!(reg.lexers[279].config().name, "Caddyfile");
        assert_eq!(reg.lexers[296].config().name, "YAML+Jinja");
        let name = |f: &str| reg.match_filename(f).map(|l| l.config().name.clone());
        assert_eq!(name("main.go").as_deref(), Some("Go"));
        assert_eq!(name("oauth.ts").as_deref(), Some("TypeScript"));
        assert_eq!(name("App.svelte").as_deref(), Some("Svelte"));
        assert_eq!(name("x.php").as_deref(), Some("PHP"));
        assert_eq!(name("README.md").as_deref(), Some("markdown"));
        assert_eq!(name("config.yaml.bak").as_deref(), Some("YAML"));
        assert_eq!(name("NOTES"), None);
        assert_eq!(
            reg.get("xml").map(|l| l.config().name.clone()).as_deref(),
            Some("XML")
        );
        assert_eq!(
            reg.get("as").map(|l| l.config().name.clone()).as_deref(),
            Some("ActionScript")
        );
        assert_eq!(
            reg.get("test.svg")
                .map(|l| l.config().name.clone())
                .as_deref(),
            Some("XML")
        );
    }
}
