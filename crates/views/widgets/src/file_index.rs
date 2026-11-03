//! The file index as the chat's `@` menu uses it: rank files through the backend index
//! (`SearchFiles`), normalize the hits, and toast a failure. A newer search replaces the task
//! holding an older one, which cancels it (`SEARCH_CANCELLED`).

use std::collections::BTreeSet;

use gpui_kit::{App, Task};
use recur_domain::FileSearchHit as RawHit;
use recur_services::Services;
use recur_ui::widgets::toasts::ToastKind;
use recur_view_core::toast;

/// One ranked file or directory.
#[derive(Clone, Debug, PartialEq)]
pub struct FileSearchHit {
    pub item: String,
    pub score: i64,
    pub label: String,
    pub dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSearchResult {
    pub hits: Vec<FileSearchHit>,
    pub building: bool,
    pub ready: bool,
}

/// `mapFileSearchHits`: trailing slashes trimmed; a path is a directory when flagged, when it
/// ended in `/`, or when another hit sits under it.
pub fn map_file_search_hits(raw: &[RawHit]) -> Vec<FileSearchHit> {
    let rows: Vec<(String, i64, bool)> = raw
        .iter()
        .map(|h| {
            (
                h.path.trim_end_matches('/').to_string(),
                h.score,
                h.dir || h.path.ends_with('/'),
            )
        })
        .filter(|h| !h.0.is_empty())
        .collect();
    let mut parents = BTreeSet::new();
    for (path, _, _) in &rows {
        let mut p = path.as_str();
        while let Some(i) = p.rfind('/') {
            if i == 0 {
                break;
            }
            p = &p[..i];
            parents.insert(p.to_string());
        }
    }
    rows.into_iter()
        .map(|(path, score, flagged)| FileSearchHit {
            dir: flagged || parents.contains(&path),
            label: path.clone(),
            item: path,
            score,
        })
        .collect()
}

/// `searchFiles`: an empty workspace searches the project checkout.
pub fn search_files(
    services: &Services,
    project_id: &str,
    workspace_id: &str,
    query: &str,
    limit: i64,
    cx: &mut App,
) -> Task<FileSearchResult> {
    if project_id.is_empty() && workspace_id.is_empty() {
        return Task::ready(FileSearchResult::default());
    }
    let services = services.clone();
    let (p, w, q) = (
        project_id.to_string(),
        workspace_id.to_string(),
        query.to_string(),
    );
    cx.spawn(
        async move |cx| match services.code.search_files(&p, &w, &q, limit, false).await {
            Ok(res) => FileSearchResult {
                hits: map_file_search_hits(res.hits.as_deref().unwrap_or_default()),
                building: res.building,
                ready: res.ready,
            },
            Err(err) => {
                let msg = format!("Search files failed: {}", err.message);
                cx.update(|cx| toast(ToastKind::Error, msg, cx));
                FileSearchResult::default()
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(path: &str, dir: bool) -> RawHit {
        RawHit {
            path: path.into(),
            score: 1,
            dir,
        }
    }

    #[test]
    fn maps_dirs_from_flags_slashes_and_parents() {
        let hits = map_file_search_hits(&[
            raw("src/", false),
            raw("src/lib/a.ts", false),
            raw("src/lib", false),
            raw("docs", true),
            raw("", false),
        ]);
        let got: Vec<(&str, bool)> = hits.iter().map(|h| (h.item.as_str(), h.dir)).collect();
        assert_eq!(
            got,
            [
                ("src", true),
                ("src/lib/a.ts", false),
                ("src/lib", true),
                ("docs", true)
            ]
        );
    }
}
