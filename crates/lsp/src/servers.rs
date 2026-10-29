//! Which language server runs for a language: the builtin PATH registry, bundle
//! `[[languageServer]]` overrides, and the servers the user turned off in
//! Settings.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use parking_lot::RwLock;
use recur_bundles::LanguageServer;

/// How to launch a language server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerSpec {
    /// A stable session key (usually the server id).
    pub key: String,
    /// Editor / LSP language ids this server handles.
    pub languages: Vec<String>,
    /// Looked up on PATH unless absolute.
    pub command: String,
    pub args: Vec<String>,
    /// A pin-registry id (`gopls`); empty means PATH only.
    pub managed: String,
    /// Repo-relative path globs this server is limited to; empty means every
    /// file of `languages`.
    pub file_glob: Vec<String>,
    /// `KEY=VAL` extras for the child environment.
    pub env: Vec<String>,
    /// `bundle` or `builtin`.
    pub source: String,
    /// The contributing bundle when `source` is `bundle`.
    pub bundle_name: String,
}

fn builtin(key: &str, langs: &[&str], command: &str, args: &[&str]) -> ServerSpec {
    ServerSpec {
        key: key.into(),
        languages: langs.iter().map(|s| s.to_string()).collect(),
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        source: "builtin".into(),
        ..Default::default()
    }
}

/// The v1 PATH registry (soft-degrade when the binary is missing). Several
/// servers may share a language (basedpyright and ruff for python).
pub fn builtin_servers() -> Vec<ServerSpec> {
    vec![
        builtin(
            "gopls",
            &["go", "go.mod", "go.work", "go.sum", "gomod", "gowork"],
            "gopls",
            &["serve"],
        ),
        builtin("rust-analyzer", &["rust"], "rust-analyzer", &[]),
        builtin(
            "typescript-language-server",
            &[
                "typescript",
                "javascript",
                "typescriptreact",
                "javascriptreact",
                "tsx",
                "jsx",
                "vue",
            ],
            "typescript-language-server",
            &["--stdio"],
        ),
        builtin(
            "basedpyright",
            &["python"],
            "basedpyright-langserver",
            &["--stdio"],
        ),
        builtin("ruff", &["python"], "ruff", &["server"]),
        builtin("clangd", &["c", "cpp"], "clangd", &[]),
        builtin("jdtls", &["java"], "jdtls", &[]),
        builtin("omnisharp", &["csharp"], "omnisharp", &["-lsp"]),
        builtin(
            "haskell-language-server",
            &["haskell"],
            "haskell-language-server-wrapper",
            &["--lsp"],
        ),
        builtin(
            "vscode-json-language-server",
            &["json", "jsonc"],
            "vscode-json-language-server",
            &["--stdio"],
        ),
        builtin(
            "vscode-html-language-server",
            &["html"],
            "vscode-html-language-server",
            &["--stdio"],
        ),
        builtin(
            "vscode-css-language-server",
            &["css", "scss", "less"],
            "vscode-css-language-server",
            &["--stdio"],
        ),
        builtin(
            "yaml-language-server",
            &["yaml"],
            "yaml-language-server",
            &["--stdio"],
        ),
        builtin("taplo", &["toml"], "taplo", &["lsp", "stdio"]),
        builtin(
            "bash-language-server",
            &["shellscript", "shell", "bash"],
            "bash-language-server",
            &["start"],
        ),
        builtin(
            "svelte-language-server",
            &["svelte"],
            "svelteserver",
            &["--stdio"],
        ),
        builtin("marksman", &["markdown"], "marksman", &["server"]),
    ]
}

/// Bundle-contributed servers and the disabled set. The [`crate::Manager`] owns
/// one.
#[derive(Debug, Default)]
pub struct ServerRegistry {
    inner: RwLock<RegistryState>,
}

#[derive(Debug, Default)]
struct RegistryState {
    bundle: Vec<ServerSpec>,
    disabled: HashSet<String>,
}

impl ServerRegistry {
    /// Replaces the bundle-contributed servers.
    pub fn set_bundle_servers(&self, servers: &[LanguageServer]) {
        let specs = servers
            .iter()
            .map(|s| ServerSpec {
                key: s.id.clone(),
                languages: s.languages.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                managed: s.managed.clone(),
                file_glob: s.file_glob.clone(),
                env: s.env.clone(),
                source: "bundle".into(),
                bundle_name: s.bundle_name.clone(),
            })
            .collect();
        self.inner.write().bundle = specs;
    }

    /// Marks server keys the user turned off in Settings.
    pub fn set_disabled_servers(&self, ids: &[String]) {
        self.inner.write().disabled = ids
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
    }

    /// Builtins merged with bundle servers (a bundle key replaces the
    /// builtin in place; new keys follow in bundle order).
    pub fn all_servers(&self) -> Vec<ServerSpec> {
        let bundle = self.inner.read().bundle.clone();
        let mut by_key: HashMap<String, ServerSpec> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for s in builtin_servers().into_iter().chain(bundle) {
            if !by_key.contains_key(&s.key) {
                order.push(s.key.clone());
            }
            by_key.insert(s.key.clone(), s);
        }
        order
            .into_iter()
            .filter_map(|k| by_key.remove(&k))
            .collect()
    }

    /// The first server for a language, or `None` when unsupported or
    /// disabled.
    pub fn lookup(&self, language_id: &str) -> Option<ServerSpec> {
        self.lookup_all(language_id, "").into_iter().next()
    }

    /// Every enabled server for `language_id`, filtered by `rel_path`
    /// globs when given. An empty language matches only servers whose
    /// globs claim `rel_path`, so a third-party pack can attach without a
    /// core language-id map.
    pub fn lookup_all(&self, language_id: &str, rel_path: &str) -> Vec<ServerSpec> {
        let lang = language_id.trim().to_lowercase();
        let disabled = self.inner.read().disabled.clone();
        let out: Vec<ServerSpec> = self
            .all_servers()
            .into_iter()
            .filter(|s| !disabled.contains(&s.key))
            .filter(|s| {
                if lang.is_empty() {
                    !rel_path.is_empty() && !s.file_glob.is_empty() && s.matches_path(rel_path)
                } else {
                    s.languages.contains(&lang) && s.matches_path(rel_path)
                }
            })
            .collect();
        prefer_vue_primary(out)
    }
}

/// Vue 3 needs vue-language-server as the feature client; TypeScript hosts
/// tsserver behind it.
fn prefer_vue_primary(mut specs: Vec<ServerSpec>) -> Vec<ServerSpec> {
    if let Some(i) = specs.iter().position(|s| s.key == "vue-language-server")
        && i > 0
    {
        let vue = specs.remove(i);
        specs.insert(0, vue);
    }
    specs
}

impl ServerSpec {
    fn matches_path(&self, rel_path: &str) -> bool {
        rel_path.is_empty()
            || self.file_glob.is_empty()
            || recur_bundles::match_file_glob(&self.file_glob, rel_path)
    }

    /// Whether `command` resolves on PATH (or is an existing file). Managed
    /// installs are resolved by the manager before this check.
    pub fn available(&self) -> bool {
        if self.command.is_empty() {
            return false;
        }
        if Path::new(&self.command).is_absolute() {
            return crate::uri::is_file(Path::new(&self.command));
        }
        look_path(&self.command).is_some()
    }
}

/// A name with a slash is checked as is; otherwise the first executable regular
/// file named `name` on `PATH`.
pub fn look_path(name: &str) -> Option<String> {
    if name.contains('/') {
        return is_executable(Path::new(name)).then(|| name.to_owned());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').find_map(|dir| {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = Path::new(dir).join(name);
        is_executable(&p).then(|| p.to_string_lossy().into_owned())
    })
}

/// A regular file with an execute bit.
pub(crate) fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls(id: &str, langs: &[&str], cmd: &str, glob: &[&str]) -> LanguageServer {
        LanguageServer {
            id: id.into(),
            languages: langs.iter().map(|s| s.to_string()).collect(),
            command: cmd.into(),
            file_glob: glob.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn lookup_go() {
        let r = ServerRegistry::default();
        for lang in ["go", "go.mod", "go.work", "go.sum", "gomod", "gowork"] {
            assert_eq!(r.lookup(lang).unwrap().key, "gopls", "{lang}");
        }
    }

    #[test]
    fn lookup_python_both() {
        let r = ServerRegistry::default();
        let all = r.lookup_all("python", "");
        assert_eq!(
            (all[0].key.as_str(), all[1].key.as_str()),
            ("basedpyright", "ruff")
        );
        r.set_disabled_servers(&["basedpyright".into()]);
        assert_eq!(r.lookup("python").unwrap().key, "ruff");
    }

    #[test]
    fn lookup_path_only() {
        let r = ServerRegistry::default();
        for (lang, key) in [
            ("c", "clangd"),
            ("java", "jdtls"),
            ("csharp", "omnisharp"),
            ("haskell", "haskell-language-server"),
            ("typescriptreact", "typescript-language-server"),
            ("javascriptreact", "typescript-language-server"),
        ] {
            assert_eq!(r.lookup(lang).unwrap().key, key);
        }
    }

    #[test]
    fn bundle_server_overrides_builtin() {
        let r = ServerRegistry::default();
        let mut s = ls("gopls", &["go"], "/tmp/custom-gopls", &[]);
        s.managed = "gopls".into();
        r.set_bundle_servers(&[s]);
        let got = r.lookup("go").unwrap();
        assert_eq!(
            (got.command.as_str(), got.source.as_str()),
            ("/tmp/custom-gopls", "bundle")
        );
        r.set_disabled_servers(&["gopls".into()]);
        assert!(r.lookup("go").is_none());
    }

    #[test]
    fn bundle_file_glob_filters_lookup() {
        let r = ServerRegistry::default();
        r.set_bundle_servers(&[ls(
            "basedpyright",
            &["python"],
            "basedpyright-langserver",
            &["src/*.py"],
        )]);
        assert_eq!(r.lookup_all("python", "src/app.py")[0].key, "basedpyright");
        assert!(
            r.lookup_all("python", "tests/test_app.py")
                .iter()
                .all(|s| s.key != "basedpyright")
        );
    }

    #[test]
    fn lookup_vue_pairs_typescript() {
        let r = ServerRegistry::default();
        r.set_bundle_servers(&[
            ls(
                "vue-language-server",
                &["vue"],
                "vue-language-server",
                &["*.vue"],
            ),
            ls(
                "typescript-language-server",
                &["typescript", "javascript", "vue"],
                "typescript-language-server",
                &["*.{js,ts,vue}"],
            ),
        ]);
        let keys: Vec<String> = r
            .lookup_all("vue", "App.vue")
            .into_iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(keys, ["vue-language-server", "typescript-language-server"]);
        assert!(
            r.lookup_all("vue", "util.ts")
                .iter()
                .all(|s| s.key != "vue-language-server")
        );
    }

    #[test]
    fn bundle_env_copied_to_spec() {
        let r = ServerRegistry::default();
        let mut s = ls("gopls", &["go"], "gopls", &[]);
        s.env = vec!["GOPACKAGESDRIVER=off".into()];
        r.set_bundle_servers(&[s]);
        assert_eq!(r.lookup("go").unwrap().env, ["GOPACKAGESDRIVER=off"]);
    }

    #[test]
    fn lookup_all_file_glob_without_language() {
        let r = ServerRegistry::default();
        r.set_bundle_servers(&[ls(
            "solidity-language-server",
            &["solidity"],
            "solidity-language-server",
            &["*.sol"],
        )]);
        let all = r.lookup_all("", "contracts/Token.sol");
        assert!(all.len() == 1 && all[0].key == "solidity-language-server");
        assert!(r.lookup_all("", "README.md").is_empty());
        assert!(
            r.lookup_all("", "main.go").is_empty(),
            "builtins without globs must not match"
        );
    }
}
