//! `lib/file-index.ts`: normalizing Quick Open / @-mention hits.

use recur_domain::FileSearchHit;

/// One normalized hit: `item` and `label` are the path without a trailing
/// slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    pub item: String,
    pub score: i64,
    pub label: String,
    pub dir: bool,
}

/// Normalizes ranked hits: a path is a directory when the backend flagged
/// it, it ends in `/`, or it is the parent of another hit. Empty paths drop.
pub fn map_file_search_hits(raw: &[FileSearchHit]) -> Vec<FileHit> {
    let rows: Vec<(String, i64, bool)> = raw
        .iter()
        .map(|h| {
            (
                h.path.trim_end_matches('/').to_string(),
                h.score,
                h.dir || h.path.ends_with('/'),
            )
        })
        .filter(|(p, _, _)| !p.is_empty())
        .collect();
    let mut parents = std::collections::HashSet::new();
    for (path, _, _) in &rows {
        let mut p = path.as_str();
        while let Some(i) = p.rfind('/').filter(|&i| i > 0) {
            p = &p[..i];
            parents.insert(p.to_string());
        }
    }
    rows.into_iter()
        .map(|(path, score, flagged)| FileHit {
            dir: flagged || parents.contains(&path),
            item: path.clone(),
            label: path,
            score,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, score: i64, dir: bool) -> FileSearchHit {
        FileSearchHit {
            path: path.into(),
            score,
            dir,
        }
    }

    fn dir_of<'a>(hits: &'a [FileHit], item: &str) -> Option<bool> {
        hits.iter().find(|h| h.item == item).map(|h| h.dir)
    }

    #[test]
    fn marks_a_path_as_a_directory_when_flagged() {
        let hits = map_file_search_hits(&[hit("src/", 10, true), hit("src/app.ts", 8, false)]);
        assert_eq!(dir_of(&hits, "src"), Some(true));
        assert_eq!(dir_of(&hits, "src/app.ts"), Some(false));
    }

    #[test]
    fn infers_directories_from_parent_paths() {
        let hits = map_file_search_hits(&[hit("src", 10, false), hit("src/app.ts", 8, false)]);
        assert_eq!(dir_of(&hits, "src"), Some(true));
        assert_eq!(dir_of(&hits, "src/app.ts"), Some(false));
    }

    #[test]
    fn treats_a_trailing_slash_as_a_directory() {
        assert_eq!(
            map_file_search_hits(&[hit("internal/", 5, false), hit("", 1, false)]),
            [FileHit {
                item: "internal".into(),
                score: 5,
                label: "internal".into(),
                dir: true
            }]
        );
    }
}
