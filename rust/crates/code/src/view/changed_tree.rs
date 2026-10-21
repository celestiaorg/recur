//! `lib/changed-tree.ts`: the Changed explorer and review TOC tree built
//! from a flat list of changed paths.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use recur_domain::TreeEntry;

use crate::collate::locale_compare;

struct Node {
    entry: TreeEntry,
    kids: Level,
}

/// Insertion-ordered children with a name index (JavaScript's `Map`).
#[derive(Default)]
struct Level {
    nodes: Vec<Node>,
    by_name: HashMap<String, usize>,
}

/// The nested structure implied by repo-relative paths, directories first
/// then names in `localeCompare` order (the Observatory Changed explorer's
/// sort).
pub fn build_tree_from_paths(paths: &[String]) -> Vec<TreeEntry> {
    let leaves: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut roots = Level::default();
    for p in paths {
        let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        let mut level = &mut roots;
        let mut acc = String::new();
        for (i, part) in parts.iter().enumerate() {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            let idx = match level.by_name.get(*part) {
                Some(&idx) => idx,
                None => {
                    level.by_name.insert(part.to_string(), level.nodes.len());
                    level.nodes.push(Node {
                        entry: TreeEntry {
                            name: part.to_string(),
                            path: acc.clone(),
                            dir: i < parts.len() - 1,
                            ..Default::default()
                        },
                        kids: Level::default(),
                    });
                    level.nodes.len() - 1
                }
            };
            level = &mut level.nodes[idx].kids;
        }
    }
    to_entries(roots, &leaves)
}

fn to_entries(level: Level, leaves: &HashSet<&str>) -> Vec<TreeEntry> {
    let mut out: Vec<TreeEntry> = level
        .nodes
        .into_iter()
        .map(|n| {
            let mut children = to_entries(n.kids, leaves);
            if children.is_empty() {
                return n.entry;
            }
            // Git can list a path and its descendants when a file became a
            // directory (or back): keep the leaf beside the new children.
            if leaves.contains(n.entry.path.as_str()) {
                children.push(TreeEntry {
                    name: n.entry.name.clone(),
                    path: n.entry.path.clone(),
                    dir: false,
                    ..Default::default()
                });
                children.sort_by(compare_tree_entry);
            }
            TreeEntry {
                dir: true,
                children,
                ..n.entry
            }
        })
        .collect();
    out.sort_by(compare_tree_entry);
    out
}

fn compare_tree_entry(a: &TreeEntry, b: &TreeEntry) -> Ordering {
    b.dir
        .cmp(&a.dir)
        .then_with(|| locale_compare(&a.name, &b.name))
}

/// One visible row of a collapsible tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRow<'a> {
    pub entry: &'a TreeEntry,
    pub depth: usize,
}

/// Visible depth-first rows (directories included; collapsed directories
/// omit their children).
pub fn flatten_visible_tree<'a>(
    tree: &'a [TreeEntry],
    collapsed: &HashSet<String>,
) -> Vec<TreeRow<'a>> {
    fn walk<'a>(
        nodes: &'a [TreeEntry],
        depth: usize,
        collapsed: &HashSet<String>,
        out: &mut Vec<TreeRow<'a>>,
    ) {
        for n in nodes {
            out.push(TreeRow { entry: n, depth });
            if n.dir && !collapsed.contains(&n.path) {
                walk(&n.children, depth + 1, collapsed, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, 0, collapsed, &mut out);
    out
}

/// `tree` with at most `limit` visible rows, same order as
/// [`flatten_visible_tree`]. `None` is no limit. The input comes back
/// borrowed when nothing was cut.
pub fn cap_visible_tree<'a>(
    tree: &'a [TreeEntry],
    collapsed: &HashSet<String>,
    limit: Option<usize>,
) -> Cow<'a, [TreeEntry]> {
    let Some(limit) = limit else {
        return Cow::Borrowed(tree);
    };
    if limit == 0 {
        return Cow::Owned(Vec::new());
    }
    fn take(
        nodes: &[TreeEntry],
        collapsed: &HashSet<String>,
        left: &mut usize,
        truncated: &mut bool,
    ) -> Vec<TreeEntry> {
        let mut out = Vec::new();
        for n in nodes {
            if *left == 0 {
                *truncated = true;
                break;
            }
            *left -= 1;
            if n.dir && !collapsed.contains(&n.path) && !n.children.is_empty() {
                out.push(TreeEntry {
                    children: take(&n.children, collapsed, left, truncated),
                    ..n.clone()
                });
            } else {
                out.push(n.clone());
            }
        }
        out
    }
    let (mut left, mut truncated) = (limit, false);
    let next = take(tree, collapsed, &mut left, &mut truncated);
    if truncated {
        Cow::Owned(next)
    } else {
        Cow::Borrowed(tree)
    }
}

/// File paths under `dir` (`a` matches `a/b`, not `ab`).
pub fn paths_under_dir(dir: &str, paths: &[String]) -> Vec<String> {
    if dir.is_empty() {
        return paths.to_vec();
    }
    let prefix = format!("{dir}/");
    paths
        .iter()
        .filter(|p| p.starts_with(&prefix))
        .cloned()
        .collect()
}

/// File paths in the depth-first order the Changed tree and review TOC show.
pub fn order_changed_paths(paths: &[String]) -> Vec<String> {
    fn walk(nodes: &[TreeEntry], out: &mut Vec<String>) {
        for n in nodes {
            if n.dir {
                walk(&n.children, out);
            } else {
                out.push(n.path.clone());
            }
        }
    }
    let mut out = Vec::new();
    walk(&build_tree_from_paths(paths), &mut out);
    out
}

/// Reorders files to the Changed-tree / TOC order instead of git's byte order.
pub fn sort_files_by_changed_tree<T: Clone>(files: &[T], path_of: impl Fn(&T) -> &str) -> Vec<T> {
    if files.len() < 2 {
        return files.to_vec();
    }
    let paths: Vec<String> = files.iter().map(|f| path_of(f).to_string()).collect();
    let rank: HashMap<String, usize> = order_changed_paths(&paths)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p, i))
        .collect();
    let mut out = files.to_vec();
    out.sort_by_key(|f| rank.get(path_of(f)).copied().unwrap_or(0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn names(t: &[TreeEntry]) -> Vec<&str> {
        t.iter().map(|e| e.name.as_str()).collect()
    }

    fn rows(t: &[TreeEntry], collapsed: &[&str]) -> Vec<(String, bool, usize)> {
        let collapsed: HashSet<String> = collapsed.iter().map(|c| c.to_string()).collect();
        flatten_visible_tree(t, &collapsed)
            .into_iter()
            .map(|r| (r.entry.path.clone(), r.entry.dir, r.depth))
            .collect()
    }

    #[test]
    fn build_empty() {
        assert!(build_tree_from_paths(&[]).is_empty());
    }

    #[test]
    fn build_nests_and_sorts_dirs_first() {
        let tree = build_tree_from_paths(&s(&["b.txt", "a/z.go", "a/b.go"]));
        assert_eq!(names(&tree), ["a", "b.txt"]);
        assert!(tree[0].dir && !tree[1].dir);
        assert_eq!(names(&tree[0].children), ["b.go", "z.go"]);
        assert_eq!(tree[1].path, "b.txt");
    }

    #[test]
    fn build_single_nested_file() {
        let tree = build_tree_from_paths(&s(&["src/main.go"]));
        assert_eq!(tree.len(), 1);
        assert_eq!(
            (tree[0].name.as_str(), tree[0].path.as_str(), tree[0].dir),
            ("src", "src", true)
        );
        assert_eq!(
            (tree[0].children[0].path.as_str(), tree[0].children[0].dir),
            ("src/main.go", false)
        );
    }

    #[test]
    fn build_sorts_with_locale_compare() {
        // Underscore sorts before dot under localeCompare.
        let tree = build_tree_from_paths(&s(&[
            "internal/store/seed.go",
            "internal/store/seed_prs.go",
        ]));
        assert_eq!(
            names(&tree[0].children[0].children),
            ["seed_prs.go", "seed.go"]
        );
    }

    #[test]
    fn build_keeps_a_type_changed_file_beside_descendants() {
        for paths in [s(&["a", "a/b.txt"]), s(&["a/b.txt", "a"])] {
            let tree = build_tree_from_paths(&paths);
            assert_eq!(tree.len(), 1);
            assert_eq!((tree[0].path.as_str(), tree[0].dir), ("a", true));
            let kids: Vec<(&str, bool)> = tree[0]
                .children
                .iter()
                .map(|e| (e.path.as_str(), e.dir))
                .collect();
            assert_eq!(kids, [("a", false), ("a/b.txt", false)]);
        }
    }

    #[test]
    fn flatten_dfs_and_collapse() {
        let tree = build_tree_from_paths(&s(&["b.txt", "a/z.go", "a/b.go"]));
        assert_eq!(
            rows(&tree, &[]),
            [
                ("a".into(), true, 0),
                ("a/b.go".into(), false, 1),
                ("a/z.go".into(), false, 1),
                ("b.txt".into(), false, 0)
            ]
        );
        let tree = build_tree_from_paths(&s(&["a/b.go", "a/c.go", "d.txt"]));
        let paths: Vec<String> = rows(&tree, &["a"]).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, ["a", "d.txt"]);
        assert_eq!(
            rows(&build_tree_from_paths(&s(&["a", "a/b.txt"])), &[]),
            [
                ("a".into(), true, 0),
                ("a".into(), false, 1),
                ("a/b.txt".into(), false, 1)
            ]
        );
    }

    #[test]
    fn cap_keeps_structure_and_limit() {
        let none = HashSet::new();
        let tree = build_tree_from_paths(&s(&["a/b.go", "a/c.go", "d.txt"]));
        let capped = cap_visible_tree(&tree, &none, Some(3));
        assert!(matches!(capped, Cow::Owned(_)));
        let paths: Vec<String> = rows(&capped, &[]).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, ["a", "a/b.go", "a/c.go"]);

        let tree = build_tree_from_paths(&s(&["a/b.go", "d.txt"]));
        assert!(matches!(
            cap_visible_tree(&tree, &none, Some(10)),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            cap_visible_tree(&tree, &none, None),
            Cow::Borrowed(_)
        ));
        assert!(cap_visible_tree(&tree, &none, Some(0)).is_empty());

        let tree = build_tree_from_paths(&s(&["a/1.go", "a/2.go", "b/1.go", "b/2.go"]));
        let collapsed: HashSet<String> = ["a".to_string()].into();
        let capped = cap_visible_tree(&tree, &collapsed, Some(3));
        let paths: Vec<String> = rows(&capped, &["a"]).into_iter().map(|r| r.0).collect();
        assert_eq!(paths, ["a", "b", "b/1.go"]);
    }

    #[test]
    fn paths_under_dir_ignores_same_prefix_sibling() {
        assert_eq!(
            paths_under_dir("a", &s(&["a/b.go", "a/c/d.go", "ab/e.go", "a"])),
            ["a/b.go", "a/c/d.go"]
        );
        assert_eq!(paths_under_dir("", &s(&["x"])), ["x"]);
    }

    #[test]
    fn order_changed_paths_is_tree_dfs() {
        assert_eq!(
            order_changed_paths(&s(&["b.txt", "a/z.go", "a/b.go"])),
            ["a/b.go", "a/z.go", "b.txt"]
        );
        assert_eq!(
            order_changed_paths(&s(&[
                "internal/store/seed.go",
                "internal/store/seed_prs.go"
            ])),
            ["internal/store/seed_prs.go", "internal/store/seed.go"]
        );
    }

    #[test]
    fn sort_files_by_changed_tree_reorders() {
        let files = s(&["internal/store/seed.go", "internal/store/seed_prs.go"]);
        assert_eq!(
            sort_files_by_changed_tree(&files, |f| f.as_str()),
            order_changed_paths(&files)
        );
    }
}
