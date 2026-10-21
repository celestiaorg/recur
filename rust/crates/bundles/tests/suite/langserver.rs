//! Port of `langserver_test.go`.

use std::collections::HashMap;

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_language_server() {
    let (_g, dir) = temp_dir();
    let parsed = parse_manifest_full(
        &dir,
        b"\n[bundle]\nname = \"Go\"\nid = \"go-bundle\"\n[[languageServer]]\nid = \"gopls\"\nlanguages = [\"go\", \"GoMod\"]\ncommand = \"gopls\"\nargs = [\"serve\"]\nmanaged = \"gopls\"\n",
    )
    .unwrap();
    assert_eq!(parsed.meta.name, "Go");
    assert!(parsed.commands.is_empty() && parsed.prompts.is_empty());
    assert_eq!(parsed.language_servers.len(), 1);
    let s = &parsed.language_servers[0];
    assert_eq!(
        (s.id.as_str(), s.managed.as_str(), s.command.as_str()),
        ("gopls", "gopls", "gopls")
    );
    assert_eq!(s.languages, ["go", "gomod"]);
    assert_eq!(s.args, ["serve"]);
}

#[test]
fn discover_language_server_user_wins() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    write_bundle(
        &def,
        "Go",
        "[bundle]\nname = \"Go\"\nid = \"go-default\"\n[[languageServer]]\nid = \"gopls\"\nlanguages = [\"go\"]\ncommand = \"gopls-default\"\n",
    );
    write_bundle(
        &user,
        "Go",
        "[bundle]\nname = \"Go User\"\nid = \"go-user\"\n[[languageServer]]\nid = \"gopls\"\nlanguages = [\"go\"]\ncommand = \"gopls-user\"\nmanaged = \"gopls\"\n",
    );
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    assert_eq!(cat.language_servers.len(), 1);
    let s = &cat.language_servers[0];
    assert_eq!(s.command, "gopls-user");
    assert_eq!(s.managed, "gopls");
    assert_eq!(s.source_root, "user");
    assert!(find_language_server(&cat.language_servers, "gopls").is_some());
}

#[test]
fn discover_shipped_language_servers() {
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    let managed: HashMap<&str, bool> = cat
        .language_servers
        .iter()
        .map(|s| (s.id.as_str(), !s.managed.is_empty()))
        .collect();
    for id in [
        "basedpyright",
        "bash-language-server",
        "gopls",
        "marksman",
        "ruff",
        "rust-analyzer",
        "solidity-language-server",
        "svelte-language-server",
        "taplo",
        "typescript-language-server",
        "vscode-css-language-server",
        "vscode-html-language-server",
        "vscode-json-language-server",
        "vue-language-server",
        "yaml-language-server",
    ] {
        let Some(m) = managed.get(id) else {
            panic!("missing shipped language server {id}");
        };
        if id != "solidity-language-server" {
            assert!(m, "shipped language server {id} is missing managed pin");
        }
    }
}
