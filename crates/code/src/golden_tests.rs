//! Snapshot tests over `tests/golden/go_code.json` and `go_copyfiles.json`.
//! Edit the fixture by hand when the behavior changes on purpose.

use std::collections::BTreeMap;
use std::fs;

use recur_domain::path;
use recur_domain::{
    FileSearchHit, OutlineSymbol, SearchOpts, SearchResult, SymbolDef, SymbolRef, TreeEntry,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::filesearch::{compile_excludes, exclude_to_rg_globs, excluded};
use crate::search::match_simple_glob;
use crate::testutil::tmp;
use crate::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Golden {
    files: BTreeMap<String, String>,
    outline: BTreeMap<String, Option<Vec<OutlineSymbol>>>,
    search: Vec<SearchCase>,
    defs: BTreeMap<String, Option<Vec<SymbolDef>>>,
    refs: BTreeMap<String, Option<Vec<SymbolRef>>>,
    paths: Vec<String>,
    rank: BTreeMap<String, Vec<FileSearchHit>>,
    rank_dirs: BTreeMap<String, Vec<FileSearchHit>>,
    excluded: BTreeMap<String, BTreeMap<String, bool>>,
    rg_globs: Vec<String>,
    glob: BTreeMap<String, BTreeMap<String, bool>>,
    frontmatter: BTreeMap<String, FrontCase>,
    idents: BTreeMap<String, Option<Vec<String>>>,
    simplify: String,
    tree: Vec<TreeEntry>,
    list_files: Vec<String>,
}

#[derive(Deserialize)]
struct SearchCase {
    query: String,
    opts: SearchOpts,
    result: SearchResult,
    error: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrontCase {
    description: String,
    user_invocable: bool,
    disable_model_invocation: bool,
    body: String,
    ok: bool,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("../tests/golden/go_code.json")).expect("golden json")
}

fn materialize(g: &Golden) -> tempfile::TempDir {
    let root = tmp();
    for (rel, body) in &g.files {
        crate::testutil::write(root.path(), rel, body);
    }
    root
}

#[test]
fn outline_matches_go() {
    let g = golden();
    let root = materialize(&g);
    for (rel, want) in &g.outline {
        let got = outline(root.path(), rel).unwrap();
        assert_eq!(&got, want.as_ref().unwrap_or(&Vec::new()), "outline({rel})");
    }
    assert_eq!(
        g.outline.len(),
        g.files.len(),
        "every fixture has an outline"
    );
}

#[test]
fn search_matches_go() {
    let g = golden();
    let root = materialize(&g);
    for c in &g.search {
        match search_opts(&CancellationToken::new(), root.path(), &c.query, &c.opts) {
            Ok(got) => {
                assert!(c.error.is_empty(), "{:?}: want error {}", c.query, c.error);
                assert_eq!(got, c.result, "search {:?} {:?}", c.query, c.opts);
            }
            Err(e) => {
                assert!(!c.error.is_empty(), "{:?}: unexpected error {e}", c.query);
                assert!(
                    e.to_string()
                        .starts_with("invalid regex: regex parse error")
                );
            }
        }
    }
}

#[test]
fn defs_and_refs_match_go() {
    let g = golden();
    let root = materialize(&g);
    let never = CancellationToken::new();
    for (name, want) in &g.defs {
        let got = find_defs(&never, root.path(), name).unwrap();
        assert_eq!(&got, want.as_ref().unwrap_or(&Vec::new()), "defs({name})");
    }
    for (name, want) in &g.refs {
        let got = find_refs(&never, root.path(), name).unwrap();
        assert_eq!(&got, want.as_ref().unwrap_or(&Vec::new()), "refs({name})");
    }
}

#[test]
fn ranking_matches_go() {
    let g = golden();
    let bases: Vec<String> = g.paths.iter().map(|p| path::base(p)).collect();
    for (q, want) in &g.rank {
        assert_eq!(
            &rank_files(None, &g.paths, &bases, q, 8),
            want,
            "rank({q:?})"
        );
    }
    for (q, want) in &g.rank_dirs {
        assert_eq!(
            &rank_files_and_dirs(None, &g.paths, &bases, q, 8),
            want,
            "rank_dirs({q:?})"
        );
    }
}

#[test]
fn excludes_and_globs_match_go() {
    let g = golden();
    for (pat, probes) in &g.excluded {
        let set = compile_excludes(std::slice::from_ref(pat));
        for (p, want) in probes {
            assert_eq!(excluded(p, &set), *want, "excluded({p:?}) by {pat:?}");
        }
    }
    let mut pats: Vec<String> = g.excluded.keys().cloned().collect();
    // The fixture lists them in declaration order; the map sorted them.
    pats.sort_by_key(|p| {
        [
            "**/.git",
            "dist/**",
            "**/*.map",
            "build",
            "src/*.tmp",
            "a?c/**",
            "**/node_modules/**",
            "**/x/*/y",
            "docs",
        ]
        .iter()
        .position(|x| x == p)
    });
    pats.extend(["  /lead", "node_modules", "**/node_modules", "!neg"].map(String::from));
    assert_eq!(exclude_to_rg_globs(&pats), g.rg_globs);
    for (pat, probes) in &g.glob {
        for (p, want) in probes {
            assert_eq!(match_simple_glob(pat, p), *want, "glob {pat:?} on {p:?}");
        }
    }
}

#[test]
fn skill_frontmatter_matches_go() {
    let g = golden();
    let dir = tmp();
    let bodies = [
        (
            "plain",
            "---\nname: x\ndescription: hello   world\n---\n\n# Body\n",
        ),
        (
            "folded",
            "---\ndescription: >-\n  line one\n\n  line two\nuser-invocable: no\n---\nbody",
        ),
        (
            "literal",
            "---\ndescription: |\n\tTabbed line\n  spaced\nnext: 1\n---\n",
        ),
        (
            "quoted",
            "\u{feff}  ---\ndescription: 'It''s'\ndisable-model-invocation: TRUE\n---\nB",
        ),
        ("nofm", "# Just a body\n"),
        ("empty", "---\nname: x\n---\n"),
        ("unclosed", "---\ndescription: d\n"),
        (
            "crlf",
            "---\r\ndescription: crlf desc\r\nuser-invocable: \"false\"\r\n---\r\nbody\r\n",
        ),
    ];
    assert_eq!(bodies.len(), g.frontmatter.len());
    for (name, body) in bodies {
        let p = dir.path().join(format!("{name}.md"));
        fs::write(&p, body).unwrap();
        let p = p.to_str().unwrap();
        let want = &g.frontmatter[name];
        let got = crate::skills::read_skill_meta(p);
        assert_eq!(
            got,
            (
                want.description.clone(),
                want.user_invocable,
                want.disable_model_invocation,
                want.body.clone(),
                want.ok
            ),
            "frontmatter {name}"
        );
    }
}

#[test]
fn idents_tree_and_listing_match_go() {
    let g = golden();
    for (line, want) in &g.idents {
        assert_eq!(
            &extract_idents(line),
            want.as_ref().unwrap_or(&Vec::new()),
            "{line}"
        );
    }
    let root = materialize(&g);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let tree = rt.block_on(list_tree(root.path(), "", 3, "")).unwrap();
    assert_eq!(tree, g.tree);
    assert_eq!(
        list_files(&CancellationToken::new(), root.path()).unwrap(),
        g.list_files
    );
    assert!(!g.simplify.is_empty());
}

#[test]
fn copyfiles_match_go() {
    #[derive(Deserialize)]
    struct Copy {
        #[serde(rename = "match")]
        matches: BTreeMap<String, BTreeMap<String, bool>>,
        parse: BTreeMap<String, Option<Vec<String>>>,
    }
    let g: Copy = serde_json::from_str(include_str!("../tests/golden/go_copyfiles.json")).unwrap();
    for (pat, probes) in &g.matches {
        for (p, want) in probes {
            assert_eq!(
                copyfiles::matches(p, std::slice::from_ref(pat)),
                *want,
                "{pat:?} on {p:?}"
            );
        }
    }
    for (text, want) in &g.parse {
        assert_eq!(
            &copyfiles::parse_patterns(text),
            want.as_ref().unwrap_or(&Vec::new()),
            "{text:?}"
        );
    }
}
