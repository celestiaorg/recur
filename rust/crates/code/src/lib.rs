//! Worktree file access: tree, index, search/replace (ripgrep), outline,
//! symbols, open-in, copy-files and observatory analysis. Port of Go
//! `internal/code`, `internal/rg`, `internal/copyfiles` and `internal/observe`,
//! plus the UI's pure file rules (`lib/fuzzy.ts`, `lib/glob.ts`,
//! `lib/file-index.ts`, `lib/changed-tree.ts`, `lib/finding-paths.ts`,
//! `lib/skills.ts`).
//!
//! Paths are always validated under a root (project repo or loop worktree) so
//! the UI cannot escape the sandbox. Filesystem walks are synchronous and stop
//! at the next file when their `CancellationToken` fires; anything that runs
//! git or a subprocess is `async`.

mod changed;
pub mod collate;
pub mod copyfiles;
mod drop_allow;
mod error;
mod file;
mod fileindex;
mod files;
mod filesearch;
pub mod fuzzy;
pub mod glob;
mod gopath;
mod gore;
mod gostr;
mod langs;
pub mod observe;
mod open;
mod ops;
mod outline;
pub mod rg;
mod root;
mod scan;
mod search;
mod skills;
mod symbols;
mod tree;
pub mod view;
mod walk;

#[cfg(test)]
mod code_tests;
#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod ops_tests;
#[cfg(test)]
mod testutil;

pub use changed::{ChangedDef, changed_defs, find_refs_in_package, rank_refs_in_package};
pub use drop_allow::{allow_external_path, load_external_allow_file, reset_external_paths};
pub use error::{Error, Result};
pub use file::{
    image_at_base, image_mime, prepare_chat_image, prepare_external, prepare_serve, preview_mime,
    read_file, rewind_binary_check, write_file,
};
pub use fileindex::{
    BoxFuture, DEFAULT_MAX_INDEX_BYTES, FileIndex, IndexHub, OnUpdate, Publish, ResolveRg, Snapshot,
};
pub use files::{list_files, match_files};
pub use filesearch::{ExcludeSet, rank_files, rank_files_and_dirs, rank_hits};
pub use langs::{is_source_ext, is_source_file};
pub use open::{list_open_in_targets, open_in, open_in_editor, reveal_in_finder};
pub use ops::{abs_path, copy, create_file, mkdir, remove, rename};
pub use outline::outline;
pub use root::{rel_slash, resolve_root, resolve_root_info, safe_join};
pub use search::{replace_in_files, search, search_opts};
pub use skills::{
    list_skill_resources, list_skills, list_skills_for_menu, read_skill_body, skill_by_name,
    skill_disables_model_invocation,
};
pub use symbols::{
    SymbolIndex, changed_symbols, extract_idents, find_defs, find_refs, index_symbols,
};
pub use tree::{file_at_base, file_diff, git_status, list_tree, list_trees};
