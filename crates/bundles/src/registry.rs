//! Discovery across the shipped (`<DataDir>/Bundles`) and user
//! (`~/.recur/bundles`) roots. User wins on id collisions for every
//! contribution kind; malformed packs are skipped with a reason.

use std::collections::{BTreeMap, HashMap};
use std::io;

use recur_domain::path;

use crate::grammar::{Grammar, TerminalTheme, Theme};
use crate::langdata::LanguageData;
use crate::loops::{LoopDef, Policy, Role};
use crate::manifest::{
    Command, ExpandAction, LanguageServer, PromptAction, SearchAction, parse_manifest_full,
};
use crate::setting::Setting;
use crate::snippet::Snippet;
use crate::symbol::Symbol;
use crate::{Error, err};

/// Source label for bundles under the shipped root.
pub const SOURCE_DEFAULT: &str = "default";
/// Source label for bundles under the user root.
pub const SOURCE_USER: &str = "user";

/// The scan roots. Default loads first, then user overwrites by id.
#[derive(Debug, Clone, Default)]
pub struct DiscoverRoots {
    /// `~/.recur/bundles`
    pub user_dir: String,
    /// `<DataDir>/Bundles`
    pub default_dir: String,
}

/// A bundle directory present under a scan root but not loaded (missing or
/// unreadable manifest, schema error, …), or a prompt dropped by slash
/// collision resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoverSkip {
    /// Absolute bundle directory.
    pub dir: String,
    /// Directory basename, or the prompt slash.
    pub name: String,
    /// "user" | "default"
    pub source: String,
    pub reason: String,
}

/// The deduped discover result across all scan roots.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub commands: Vec<Command>,
    pub prompts: Vec<PromptAction>,
    pub grammars: Vec<Grammar>,
    pub themes: Vec<Theme>,
    pub terminal_themes: Vec<TerminalTheme>,
    pub language_data: Vec<LanguageData>,
    pub snippets: Vec<Snippet>,
    pub symbols: Vec<Symbol>,
    pub language_servers: Vec<LanguageServer>,
    pub settings: Vec<Setting>,
    pub searches: Vec<SearchAction>,
    pub expands: Vec<ExpandAction>,
    pub roles: Vec<Role>,
    pub loops: Vec<LoopDef>,
    pub policies: Vec<Policy>,
    pub skips: Vec<DiscoverSkip>,
}

/// Deduped commands and prompts (reload on demand, no watcher).
pub fn discover(roots: &DiscoverRoots) -> Result<(Vec<Command>, Vec<PromptAction>), Error> {
    let cat = discover_catalog(roots)?;
    Ok((cat.commands, cat.prompts))
}

/// Commands, prompts, and the skipped packs behind them.
pub type DiscoverReport = (Vec<Command>, Vec<PromptAction>, Vec<DiscoverSkip>);

/// [`discover`] plus skip reasons, so callers can explain missing commands.
pub fn discover_report(roots: &DiscoverRoots) -> Result<DiscoverReport, Error> {
    let cat = discover_catalog(roots)?;
    Ok((cat.commands, cat.prompts, cat.skips))
}

/// Explicit order first; zero sorts after every positive order.
fn theme_sort_order(order: i64) -> i64 {
    if order == 0 { 1_000_000 } else { order }
}

/// Every contribution kind across both roots. User wins on id; grammars and
/// language data then collapse onto unique scopeNames (user wins again).
pub fn discover_catalog(roots: &DiscoverRoots) -> Result<Catalog, Error> {
    let mut commands: BTreeMap<String, Command> = BTreeMap::new();
    let mut prompts: BTreeMap<String, PromptAction> = BTreeMap::new();
    let mut grammars: BTreeMap<String, Grammar> = BTreeMap::new();
    let mut themes: BTreeMap<String, Theme> = BTreeMap::new();
    let mut term_themes: BTreeMap<String, TerminalTheme> = BTreeMap::new();
    let mut lang_data: BTreeMap<String, LanguageData> = BTreeMap::new();
    let mut snippets: BTreeMap<String, Snippet> = BTreeMap::new();
    let mut symbols: BTreeMap<String, Symbol> = BTreeMap::new();
    let mut servers: BTreeMap<String, LanguageServer> = BTreeMap::new();
    let mut settings: BTreeMap<String, Setting> = BTreeMap::new();
    let mut searches: BTreeMap<String, SearchAction> = BTreeMap::new();
    let mut expands: BTreeMap<String, ExpandAction> = BTreeMap::new();
    let mut roles: BTreeMap<String, Role> = BTreeMap::new();
    let mut loops: BTreeMap<String, LoopDef> = BTreeMap::new();
    let mut policies: BTreeMap<String, Policy> = BTreeMap::new();
    let mut skips: Vec<DiscoverSkip> = Vec::new();

    macro_rules! absorb {
        ($items:expr, $map:ident, $source:expr) => {
            for mut item in $items {
                item.source_root = $source.to_string();
                $map.insert(item.id.clone(), item);
            }
        };
    }

    for (dir, source) in [
        (roots.default_dir.as_str(), SOURCE_DEFAULT),
        (roots.user_dir.as_str(), SOURCE_USER),
    ] {
        if dir.trim().is_empty() {
            continue;
        }
        let entries = match read_dir_sorted(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(err!(
                    "read bundles dir {:?}: {}",
                    dir,
                    format!("{dir}: {e}")
                ));
            }
        };
        for (name, is_dir) in entries {
            if !is_dir {
                continue;
            }
            let bundle_dir = path::join(&[dir, &name]);
            let manifest = path::join(&[&bundle_dir, "bundle.toml"]);
            let data = match std::fs::read(&manifest) {
                Ok(data) => data,
                Err(e) => {
                    skips.push(DiscoverSkip {
                        dir: bundle_dir,
                        name,
                        source: source.to_string(),
                        reason: skip_read_reason(&manifest, &e),
                    });
                    continue;
                }
            };
            let parsed = match parse_manifest_full(&bundle_dir, &data) {
                Ok(parsed) => parsed,
                Err(e) => {
                    skips.push(DiscoverSkip {
                        dir: bundle_dir,
                        name,
                        source: source.to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            absorb!(parsed.commands, commands, source);
            absorb!(parsed.prompts, prompts, source);
            absorb!(parsed.grammars, grammars, source);
            absorb!(parsed.themes, themes, source);
            absorb!(parsed.terminal_themes, term_themes, source);
            absorb!(parsed.language_data, lang_data, source);
            absorb!(parsed.snippets, snippets, source);
            absorb!(parsed.symbols, symbols, source);
            absorb!(parsed.language_servers, servers, source);
            absorb!(parsed.settings, settings, source);
            absorb!(parsed.searches, searches, source);
            absorb!(parsed.expands, expands, source);
            absorb!(parsed.roles, roles, source);
            absorb!(parsed.loops, loops, source);
            absorb!(parsed.policies, policies, source);
        }
    }

    let mut out_commands: Vec<Command> = commands.into_values().collect();
    out_commands.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.category.to_lowercase().cmp(&b.category.to_lowercase()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let (out_prompts, slash_skips) = resolve_prompt_slashes(prompts.into_values().collect());
    skips.extend(slash_skips);

    let out_grammars = collapse_by_scope(grammars.into_values().collect(), |g| {
        (g.source_root.clone(), g.id.clone(), g.scope_name.clone())
    });

    let mut out_themes: Vec<Theme> = themes.into_values().collect();
    out_themes.sort_by(|a, b| {
        theme_sort_order(a.order)
            .cmp(&theme_sort_order(b.order))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let mut out_servers: Vec<LanguageServer> = servers.into_values().collect();
    out_servers.sort_by_key(|s| s.id.to_lowercase());

    let mut out_term_themes: Vec<TerminalTheme> = term_themes.into_values().collect();
    out_term_themes.sort_by(|a, b| {
        theme_sort_order(a.order)
            .cmp(&theme_sort_order(b.order))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let out_lang_data = collapse_by_scope(lang_data.into_values().collect(), |d| {
        (d.source_root.clone(), d.id.clone(), d.scope_name.clone())
    });

    let mut out_snippets: Vec<Snippet> = snippets.into_values().collect();
    out_snippets.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| {
                a.bundle_name
                    .to_lowercase()
                    .cmp(&b.bundle_name.to_lowercase())
            })
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let mut out_symbols: Vec<Symbol> = symbols.into_values().collect();
    out_symbols.sort_by(|a, b| {
        a.bundle_name
            .to_lowercase()
            .cmp(&b.bundle_name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut out_settings: Vec<Setting> = settings.into_values().collect();
    out_settings.sort_by(|a, b| {
        a.bundle_name
            .to_lowercase()
            .cmp(&b.bundle_name.to_lowercase())
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut out_searches: Vec<SearchAction> = searches.into_values().collect();
    out_searches.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    let out_expands: Vec<ExpandAction> = expands.into_values().collect();

    // Role codes are unique across the catalog: default before user, then id.
    let mut role_list: Vec<Role> = roles.into_values().collect();
    role_list.sort_by(|a, b| {
        a.source_root
            .cmp(&b.source_root)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut code_owner: HashMap<String, String> = HashMap::new();
    let mut out_roles: Vec<Role> = Vec::with_capacity(role_list.len());
    for r in role_list {
        if let Some(prev) = code_owner.get(&r.code)
            && *prev != r.id
        {
            skips.push(DiscoverSkip {
                dir: r.bundle_dir.clone(),
                name: r.id.clone(),
                source: r.source_root.clone(),
                reason: format!("role code {} collides with {prev}", r.code),
            });
            continue;
        }
        code_owner.insert(r.code.clone(), r.id.clone());
        out_roles.push(r);
    }
    out_roles.sort_by_key(|r| r.name.to_lowercase());

    let mut out_loops: Vec<LoopDef> = loops.into_values().collect();
    out_loops.sort_by_key(|l| l.name.to_lowercase());

    let mut out_policies: Vec<Policy> = policies.into_values().collect();
    out_policies.sort_by_key(|p| p.name.to_lowercase());

    skips.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Catalog {
        commands: out_commands,
        prompts: out_prompts,
        grammars: out_grammars,
        themes: out_themes,
        terminal_themes: out_term_themes,
        language_data: out_lang_data,
        snippets: out_snippets,
        symbols: out_symbols,
        language_servers: out_servers,
        settings: out_settings,
        searches: out_searches,
        expands: out_expands,
        roles: out_roles,
        loops: out_loops,
        policies: out_policies,
        skips,
    })
}

/// Collapses id-deduped entries onto unique scopeNames: default sorts before
/// user so the user entry wins, and the result is ordered by scopeName.
fn collapse_by_scope<T>(mut items: Vec<T>, key: impl Fn(&T) -> (String, String, String)) -> Vec<T> {
    items.sort_by_key(|t| {
        let (source, id, _) = key(t);
        (source, id)
    });
    let mut by_scope: BTreeMap<String, T> = BTreeMap::new();
    for t in items {
        by_scope.insert(key(&t).2, t);
    }
    by_scope.into_values().collect()
}

/// Directory entries sorted by name. `is_dir` does not follow symlinks.
pub(crate) fn read_dir_sorted(dir: &str) -> io::Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        out.push((entry.file_name().to_string_lossy().into_owned(), is_dir));
    }
    out.sort();
    Ok(out)
}

/// Keeps one prompt per slash after id dedupe. User beats default; same
/// source breaks ties by bundle id, then prompt id (smaller wins).
fn resolve_prompt_slashes(prompts: Vec<PromptAction>) -> (Vec<PromptAction>, Vec<DiscoverSkip>) {
    let mut by_slash: BTreeMap<String, Vec<PromptAction>> = BTreeMap::new();
    for p in prompts {
        by_slash.entry(p.slash.clone()).or_default().push(p);
    }
    let mut out = Vec::with_capacity(by_slash.len());
    let mut skips = Vec::new();
    for (slash, group) in by_slash {
        let mut best = 0;
        for (i, p) in group.iter().enumerate().skip(1) {
            if prompt_slash_beats(p, &group[best]) {
                best = i;
            }
        }
        let winner = group[best].clone();
        for p in group {
            if p.id == winner.id && p.bundle_id == winner.bundle_id {
                continue;
            }
            skips.push(DiscoverSkip {
                dir: p.bundle_dir.clone(),
                name: p.slash.clone(),
                source: p.source_root.clone(),
                reason: format!(
                    "slash {:?} collides with prompt {:?} from bundle {:?} (kept {}/{})",
                    slash, winner.name, winner.bundle_name, winner.source_root, winner.id
                ),
            });
        }
        out.push(winner);
    }
    out.sort_by(|a, b| a.slash.cmp(&b.slash));
    (out, skips)
}

fn prompt_slash_beats(a: &PromptAction, b: &PromptAction) -> bool {
    if a.source_root != b.source_root {
        return a.source_root == SOURCE_USER;
    }
    if a.bundle_id != b.bundle_id {
        return a.bundle_id < b.bundle_id;
    }
    a.id < b.id
}

fn skip_read_reason(path: &str, e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "missing bundle.toml".to_string(),
        io::ErrorKind::PermissionDenied => {
            "cannot read bundle.toml (permission denied): check directory permissions".to_string()
        }
        _ => format!("cannot read bundle.toml: {path}: {e}"),
    }
}

/// The command with the given id.
pub fn find_by_id<'a>(cmds: &'a [Command], id: &str) -> Option<&'a Command> {
    cmds.iter().find(|c| c.id == id)
}

/// The prompt action with the given id.
pub fn find_prompt_by_id<'a>(prompts: &'a [PromptAction], id: &str) -> Option<&'a PromptAction> {
    prompts.iter().find(|p| p.id == id)
}

/// The prompt action with the given slash token.
pub fn find_prompt_by_slash<'a>(
    prompts: &'a [PromptAction],
    slash: &str,
) -> Option<&'a PromptAction> {
    let slash = slash.trim();
    prompts.iter().find(|p| p.slash == slash)
}

/// The language server with the given id.
pub fn find_language_server<'a>(
    servers: &'a [LanguageServer],
    id: &str,
) -> Option<&'a LanguageServer> {
    servers.iter().find(|s| s.id == id)
}
