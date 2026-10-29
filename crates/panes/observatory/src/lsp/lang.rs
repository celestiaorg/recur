//! A repo-relative path to an LSP `languageId`.

use std::sync::LazyLock;

use recur_editor::prefs::language_id_for_path;
use recur_textmate::TmCatalog;
use regex::Regex;

const JSONC_NAMES: [&str; 16] = [
    ".eslintrc",
    ".eslintrc.json",
    ".jsfmtrc",
    ".jshintrc",
    ".swcrc",
    ".hintrc",
    ".babelrc",
    ".babelrc.json",
    "babel.config.json",
    "typedoc.json",
    "settings.json",
    "launch.json",
    "tasks.json",
    "keybindings.json",
    "extensions.json",
    "devcontainer.json",
];

static TSCONFIG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[jt]sconfig(\..*)?\.json$").expect("tsconfig regex"));
static TSCONFIG_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[jt]sconfig-.*\.json$").expect("tsconfig regex"));

fn jsonc_basename(base: &str) -> bool {
    base.ends_with(".jsonc")
        || JSONC_NAMES.contains(&base)
        || TSCONFIG.is_match(base)
        || TSCONFIG_DASH.is_match(base)
}

/// `lspLanguageId`: the prefs language id (and path) to the id servers expect.
pub fn lsp_language_id(id: &str, path: &str) -> String {
    let lower = path.rsplit('/').next().unwrap_or("").to_lowercase();
    let fixed = if lower.ends_with(".tsx") {
        "typescriptreact"
    } else if lower.ends_with(".jsx") {
        "javascriptreact"
    } else if lower == "go.mod" {
        "go.mod"
    } else if lower == "go.work" {
        "go.work"
    } else if lower == "go.sum" {
        "go.sum"
    } else if lower.ends_with(".cs") {
        "csharp"
    } else if lower.ends_with(".hs") || lower.ends_with(".lhs") {
        "haskell"
    } else if lower.ends_with(".scss") {
        "scss"
    } else if lower.ends_with(".sass") {
        "sass"
    } else if lower.ends_with(".less") {
        "less"
    } else if jsonc_basename(&lower) {
        "jsonc"
    } else if lower.ends_with(".json5") {
        "json5"
    } else if lower == "dockerfile" || lower.ends_with(".dockerfile") {
        "dockerfile"
    } else if lower == "makefile" || lower == "gnumakefile" {
        "makefile"
    } else if lower.ends_with(".ps1") || lower.ends_with(".psm1") {
        "powershell"
    } else if lower.ends_with(".fish") {
        "fish"
    } else {
        ""
    };
    if !fixed.is_empty() {
        return fixed.into();
    }
    match id {
        "shell" => "shellscript".into(),
        "other" => String::new(),
        other => other.into(),
    }
}

/// `lspLanguageIdForPath`: the prefs mapping, else the matching grammar's `[[grammar]].language`.
pub fn lsp_language_id_for_path(path: &str, catalog: Option<&TmCatalog>) -> String {
    let mapped = lsp_language_id(language_id_for_path(path), path);
    if !mapped.is_empty() {
        return mapped;
    }
    let Some(catalog) = catalog else {
        return String::new();
    };
    let Some(scope) = catalog.resolve_scope_for_path(path, None) else {
        return String::new();
    };
    catalog
        .grammar_meta(&scope)
        .and_then(|g| g.language)
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_textmate::GrammarInfo;
    use std::rc::Rc;

    fn id(path: &str) -> String {
        lsp_language_id_for_path(path, None)
    }

    #[test]
    fn jsx_and_tsx_open_as_react_ids() {
        assert_eq!(id("src/App.tsx"), "typescriptreact");
        assert_eq!(id("src/App.jsx"), "javascriptreact");
        assert_eq!(id("src/util.ts"), "typescript");
        assert_eq!(id("src/util.js"), "javascript");
    }

    #[test]
    fn go_module_files_use_gopls_ids() {
        assert_eq!(id("go.mod"), "go.mod");
        assert_eq!(id("go.work"), "go.work");
        assert_eq!(id("go.sum"), "go.sum");
        assert_eq!(id("main.go"), "go");
    }

    #[test]
    fn comment_allowing_json_is_jsonc() {
        for p in [
            "tsconfig.json",
            "tsconfig.app.json",
            "tsconfig-base.json",
            "jsconfig.json",
            ".eslintrc.json",
            ".vscode/settings.json",
            "src/notes.jsonc",
        ] {
            assert_eq!(id(p), "jsonc", "{p}");
        }
        assert_eq!(id("package.json"), "json");
    }

    #[test]
    fn non_bash_files_stay_off_bash_language_server() {
        assert_eq!(id("Dockerfile"), "dockerfile");
        assert_eq!(id("Makefile"), "makefile");
        assert_eq!(id("script.ps1"), "powershell");
        assert_eq!(id("setup.fish"), "fish");
        assert_eq!(id("bin/run.sh"), "shellscript");
    }

    #[test]
    fn indented_sass_and_json5_keep_their_dialect() {
        assert_eq!(id("theme.scss"), "scss");
        assert_eq!(id("theme.sass"), "sass");
        assert_eq!(id("config.json5"), "json5");
    }

    #[test]
    fn grammar_language_covers_files_the_prefs_map_does_not_know() {
        let mut cat = TmCatalog::default();
        cat.add_grammar(
            GrammarInfo {
                id: "recur.languages.solidity".into(),
                scope_name: "source.solidity".into(),
                path: "syntaxes/solidity.tmLanguage.json".into(),
                language: Some("solidity".into()),
                file_types: Some(vec!["sol".into()]),
                bundle_name: "Languages".into(),
                source_root: "default".into(),
                ..Default::default()
            },
            Rc::new(|| Some("{}".into())),
        );
        assert_eq!(
            lsp_language_id_for_path("contracts/Token.sol", Some(&cat)),
            "solidity"
        );
        assert_eq!(
            lsp_language_id_for_path("test/Token.t.sol", Some(&cat)),
            "solidity"
        );
        assert_eq!(
            lsp_language_id_for_path("contracts/Token.sol", Some(&TmCatalog::default())),
            ""
        );
    }
}
