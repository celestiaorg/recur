//! Grammar registry: loading with dependency resolution, injections, theme.
//! A port of vscode-textmate's `main.ts` (`Registry`), `registry.ts`
//! (`SyncRegistry`) and `grammar/grammarDependencies.ts`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::grammar::{Grammar, IncludeKind, parse_include};
use crate::raw::{RawGrammar, RawRule};
use crate::theme::{RawTheme, Theme};

/// State grammars read at tokenize time: the theme can change under a
/// compiled grammar, and included grammars are looked up lazily.
pub struct RegistryShared {
    theme: RefCell<Rc<Theme>>,
    raw_grammars: RefCell<HashMap<String, Rc<RawGrammar>>>,
    injection_grammars: RefCell<HashMap<String, Vec<String>>>,
}

impl RegistryShared {
    pub(crate) fn theme(&self) -> Rc<Theme> {
        self.theme.borrow().clone()
    }

    pub(crate) fn lookup(&self, scope: &str) -> Option<Rc<RawGrammar>> {
        self.raw_grammars.borrow().get(scope).cloned()
    }

    pub(crate) fn injections(&self, scope: &str) -> Vec<String> {
        self.injection_grammars
            .borrow()
            .get(scope)
            .cloned()
            .unwrap_or_default()
    }

    fn add_grammar(&self, grammar: RawGrammar, injections: Option<Vec<String>>) {
        let scope = grammar.scope_name.clone();
        self.raw_grammars
            .borrow_mut()
            .insert(scope.clone(), Rc::new(grammar));
        if let Some(inj) = injections {
            self.injection_grammars.borrow_mut().insert(scope, inj);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadGrammarError {
    #[error("No grammar provided for <{0}>")]
    Missing(String),
    #[error("grammar {scope}: {message}")]
    Source { scope: String, message: String },
}

/// Supplies raw grammars and injection lists while a grammar loads
/// (`RegistryOptions.loadGrammar` / `getInjections`).
pub trait GrammarSource {
    fn load_grammar(&mut self, scope_name: &str) -> Result<Option<RawGrammar>, LoadGrammarError>;
    fn injections(&mut self, scope_name: &str) -> Option<Vec<String>>;
}

pub struct Registry {
    shared: Rc<RegistryShared>,
    grammars: HashMap<String, Rc<Grammar>>,
    /// Scopes whose load was attempted (vscode-textmate caches the promise,
    /// failures included, for the registry's lifetime).
    attempted: HashSet<String>,
}

impl Registry {
    pub fn new(theme: Option<&RawTheme>) -> Registry {
        Registry {
            shared: Rc::new(RegistryShared {
                theme: RefCell::new(Rc::new(Theme::from_raw(theme))),
                raw_grammars: RefCell::new(HashMap::new()),
                injection_grammars: RefCell::new(HashMap::new()),
            }),
            grammars: HashMap::new(),
            attempted: HashSet::new(),
        }
    }

    /// Change the theme. Rule stacks from before must not be reused.
    pub fn set_theme(&self, theme: &RawTheme) {
        *self.shared.theme.borrow_mut() = Rc::new(Theme::from_raw(Some(theme)));
    }

    /// Colour id -> colour. Entry 0 is empty ("unset").
    pub fn color_map(&self) -> Vec<String> {
        self.shared.theme().color_map()
    }

    /// Load `scope` and every grammar it references.
    pub fn load_grammar(
        &mut self,
        scope: &str,
        source: &mut dyn GrammarSource,
    ) -> Result<Option<Rc<Grammar>>, LoadGrammarError> {
        let mut deps = ScopeDependencyProcessor::new(scope);
        while !deps.queue.is_empty() {
            let scopes: Vec<String> = deps.queue.iter().map(|r| r.scope().to_string()).collect();
            for scope in scopes {
                self.load_single(&scope, source)?;
            }
            deps.process_queue(&self.shared)?;
        }
        Ok(self.grammar_for_scope_name(scope))
    }

    fn load_single(
        &mut self,
        scope: &str,
        source: &mut dyn GrammarSource,
    ) -> Result<(), LoadGrammarError> {
        if !self.attempted.insert(scope.to_string()) {
            return Ok(());
        }
        if let Some(g) = source.load_grammar(scope)? {
            let injections = source.injections(scope);
            self.shared.add_grammar(g, injections);
        }
        Ok(())
    }

    /// Register a raw grammar directly and compile it (`Registry.addGrammar`).
    pub fn add_grammar(&mut self, raw: RawGrammar, injections: Vec<String>) -> Rc<Grammar> {
        let scope = raw.scope_name.clone();
        self.shared.add_grammar(raw, Some(injections));
        self.grammar_for_scope_name(&scope)
            .expect("grammar was just added")
    }

    fn grammar_for_scope_name(&mut self, scope: &str) -> Option<Rc<Grammar>> {
        if let Some(g) = self.grammars.get(scope) {
            return Some(g.clone());
        }
        let raw = self.shared.lookup(scope)?;
        let g = Rc::new(Grammar::new(scope, &raw, self.shared.clone()));
        self.grammars.insert(scope.to_string(), g.clone());
        Some(g)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Reference {
    TopLevel(String),
    TopLevelRepository(String, String),
}

impl Reference {
    fn scope(&self) -> &str {
        match self {
            Reference::TopLevel(s) | Reference::TopLevelRepository(s, _) => s,
        }
    }
}

struct ScopeDependencyProcessor {
    initial: String,
    seen_full: HashSet<String>,
    seen_partial: HashSet<(String, String)>,
    queue: Vec<Reference>,
}

#[derive(Default)]
struct Collector {
    references: Vec<Reference>,
    seen: HashSet<Reference>,
    visited: HashSet<*const RawRule>,
}

impl Collector {
    fn add(&mut self, r: Reference) {
        if self.seen.insert(r.clone()) {
            self.references.push(r);
        }
    }
}

#[derive(Clone)]
struct Context {
    base: Rc<RawGrammar>,
    self_grammar: Rc<RawGrammar>,
    repository: Option<Rc<HashMap<String, Rc<RawRule>>>>,
}

impl ScopeDependencyProcessor {
    fn new(initial: &str) -> Self {
        ScopeDependencyProcessor {
            initial: initial.to_string(),
            seen_full: HashSet::from([initial.to_string()]),
            seen_partial: HashSet::new(),
            queue: vec![Reference::TopLevel(initial.to_string())],
        }
    }

    fn process_queue(&mut self, repo: &RegistryShared) -> Result<(), LoadGrammarError> {
        let q = std::mem::take(&mut self.queue);
        let mut deps = Collector::default();
        for dep in &q {
            collect_references_of_reference(dep, &self.initial, repo, &mut deps)?;
        }
        for dep in deps.references {
            match &dep {
                Reference::TopLevel(scope) => {
                    if !self.seen_full.insert(scope.clone()) {
                        continue;
                    }
                }
                Reference::TopLevelRepository(scope, rule) => {
                    if self.seen_full.contains(scope) {
                        continue;
                    }
                    if !self.seen_partial.insert((scope.clone(), rule.clone())) {
                        continue;
                    }
                }
            }
            self.queue.push(dep);
        }
        Ok(())
    }
}

fn collect_references_of_reference(
    reference: &Reference,
    base_scope: &str,
    repo: &RegistryShared,
    result: &mut Collector,
) -> Result<(), LoadGrammarError> {
    let Some(self_grammar) = repo.lookup(reference.scope()) else {
        if reference.scope() == base_scope {
            return Err(LoadGrammarError::Missing(base_scope.to_string()));
        }
        return Ok(());
    };
    let Some(base) = repo.lookup(base_scope) else {
        return Err(LoadGrammarError::Missing(base_scope.to_string()));
    };
    match reference {
        Reference::TopLevel(_) => collect_in_top_level_rule(
            &Context {
                base,
                self_grammar,
                repository: None,
            },
            result,
        ),
        Reference::TopLevelRepository(_, rule) => {
            let repository = Some(self_grammar.repository.clone());
            collect_in_top_level_repository_rule(
                rule,
                &Context {
                    base,
                    self_grammar,
                    repository,
                },
                result,
            )
        }
    }
    for injection in repo.injections(reference.scope()) {
        result.add(Reference::TopLevel(injection));
    }
    Ok(())
}

fn collect_in_top_level_repository_rule(name: &str, ctx: &Context, result: &mut Collector) {
    if let Some(rule) = ctx.repository.as_ref().and_then(|r| r.get(name)) {
        collect_in_rules(std::slice::from_ref(rule), ctx, result);
    }
}

fn collect_in_top_level_rule(ctx: &Context, result: &mut Collector) {
    let with_repo = Context {
        repository: Some(ctx.self_grammar.repository.clone()),
        ..ctx.clone()
    };
    if let Some(patterns) = &ctx.self_grammar.patterns {
        collect_in_rules(patterns, &with_repo, result);
    }
    if let Some(injections) = &ctx.self_grammar.injections {
        let rules: Vec<Rc<RawRule>> = injections.iter().map(|(_, r)| r.clone()).collect();
        collect_in_rules(&rules, &with_repo, result);
    }
}

fn collect_in_rules(rules: &[Rc<RawRule>], ctx: &Context, result: &mut Collector) {
    for rule in rules {
        if !result.visited.insert(Rc::as_ptr(rule)) {
            continue;
        }
        let pattern_repository = match &rule.repository {
            Some(own) => {
                let mut merged = ctx.repository.as_deref().cloned().unwrap_or_default();
                merged.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
                Some(Rc::new(merged))
            }
            None => ctx.repository.clone(),
        };
        if let Some(patterns) = &rule.patterns {
            collect_in_rules(
                patterns,
                &Context {
                    repository: pattern_repository.clone(),
                    ..ctx.clone()
                },
                result,
            );
        }
        let Some(include) = rule.include.as_deref().filter(|i| !i.is_empty()) else {
            continue;
        };
        match parse_include(include) {
            IncludeKind::Base => collect_in_top_level_rule(
                &Context {
                    self_grammar: ctx.base.clone(),
                    ..ctx.clone()
                },
                result,
            ),
            IncludeKind::SelfRef => collect_in_top_level_rule(ctx, result),
            IncludeKind::Relative(name) => collect_in_top_level_repository_rule(
                name,
                &Context {
                    repository: pattern_repository,
                    ..ctx.clone()
                },
                result,
            ),
            IncludeKind::TopLevel(scope) | IncludeKind::TopLevelRepository(scope, _) => {
                let known = if scope == ctx.self_grammar.scope_name {
                    Some(ctx.self_grammar.clone())
                } else if scope == ctx.base.scope_name {
                    Some(ctx.base.clone())
                } else {
                    None
                };
                match (known, parse_include(include)) {
                    (Some(g), kind) => {
                        let new_ctx = Context {
                            base: ctx.base.clone(),
                            self_grammar: g,
                            repository: pattern_repository,
                        };
                        if let IncludeKind::TopLevelRepository(_, name) = kind {
                            collect_in_top_level_repository_rule(name, &new_ctx, result);
                        } else {
                            collect_in_top_level_rule(&new_ctx, result);
                        }
                    }
                    (None, IncludeKind::TopLevelRepository(s, name)) => {
                        result.add(Reference::TopLevelRepository(
                            s.to_string(),
                            name.to_string(),
                        ));
                    }
                    (None, _) => result.add(Reference::TopLevel(scope.to_string())),
                }
            }
        }
    }
}
