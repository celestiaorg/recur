//! Whether a path is already rendered in the explorer listing (`lib/explorer-visible.ts`).

use recur_domain::TreeEntry;

/// True when `path` is in the current (filtered) tree with every ancestor directory expanded.
/// Loads and expands nothing: collapsed or filtered-out files are not visible. `child_entries`
/// gives the rendered children when they differ from `node.children` (exclude filtering).
pub fn is_path_visible_in_explorer_tree<'a>(
    roots: &'a [TreeEntry],
    path: &str,
    is_dir_expanded: impl Fn(&str) -> bool,
    child_entries: Option<&dyn Fn(&'a TreeEntry) -> Vec<&'a TreeEntry>>,
) -> bool {
    if path.is_empty() {
        return false;
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    let mut nodes: Vec<&'a TreeEntry> = roots.iter().collect();
    let mut acc = String::new();
    for (i, part) in parts.iter().enumerate() {
        if acc.is_empty() {
            acc.push_str(part);
        } else {
            acc.push('/');
            acc.push_str(part);
        }
        let Some(node) = nodes.iter().copied().find(|e| e.path == acc) else {
            return false;
        };
        if i == parts.len() - 1 {
            return true;
        }
        if !node.dir || !is_dir_expanded(&node.path) {
            return false;
        }
        nodes = match child_entries {
            Some(f) => f(node),
            None => node.children.iter().collect(),
        };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, children: Option<Vec<TreeEntry>>) -> TreeEntry {
        TreeEntry {
            name: path.rsplit('/').next().unwrap_or(path).into(),
            path: path.into(),
            dir: children.is_some(),
            children: children.unwrap_or_default(),
            ..Default::default()
        }
    }

    fn src_tree() -> Vec<TreeEntry> {
        vec![
            node(
                "src",
                Some(vec![
                    node("src/main.go", None),
                    node("src/lib", Some(vec![node("src/lib/util.ts", None)])),
                ]),
            ),
            node("README.md", None),
        ]
    }

    fn vis(tree: &[TreeEntry], path: &str, open: impl Fn(&str) -> bool) -> bool {
        is_path_visible_in_explorer_tree(tree, path, open, None)
    }

    #[test]
    fn rejects_empty_path() {
        assert!(!vis(&src_tree(), "", |_| true));
    }

    #[test]
    fn finds_a_root_level_file() {
        assert!(vis(&src_tree(), "README.md", |_| true));
    }

    #[test]
    fn finds_a_nested_file_when_ancestors_are_expanded() {
        assert!(vis(&src_tree(), "src/main.go", |_| true));
        assert!(vis(&src_tree(), "src/lib/util.ts", |_| true));
    }

    #[test]
    fn finds_a_directory_that_is_itself_in_the_listing() {
        assert!(vis(&src_tree(), "src", |_| true));
        assert!(vis(&src_tree(), "src/lib", |_| true));
    }

    #[test]
    fn hides_nested_files_when_an_ancestor_is_collapsed() {
        let open = |d: &str| d != "src/lib";
        assert!(vis(&src_tree(), "src/main.go", open));
        assert!(vis(&src_tree(), "src/lib", open));
        assert!(!vis(&src_tree(), "src/lib/util.ts", open));
    }

    #[test]
    fn hides_everything_under_a_collapsed_root_folder() {
        assert!(!vis(&src_tree(), "src/main.go", |_| false));
        assert!(vis(&src_tree(), "README.md", |_| false));
    }

    #[test]
    fn returns_false_for_paths_not_in_the_tree() {
        assert!(!vis(&src_tree(), "PROGRESS.md", |_| true));
        assert!(!vis(&src_tree(), "src/other.go", |_| true));
    }

    #[test]
    fn uses_child_entries_when_rendered_children_differ() {
        let hidden = vec![node(
            "src",
            Some(vec![node("src/secret.go", None), node("src/main.go", None)]),
        )];
        fn shown(n: &TreeEntry) -> Vec<&TreeEntry> {
            n.children
                .iter()
                .filter(|c| c.path != "src/secret.go")
                .collect()
        }
        assert!(is_path_visible_in_explorer_tree(
            &hidden,
            "src/main.go",
            |_| true,
            Some(&shown)
        ));
        assert!(!is_path_visible_in_explorer_tree(
            &hidden,
            "src/secret.go",
            |_| true,
            Some(&shown)
        ));
    }
}
