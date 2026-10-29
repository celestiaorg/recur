//! The review surface the workspace Observatory and PR detail embed.
//!
//! # Embedding
//!
//! Call [`register`] once (it installs the [`store::ReviewStore`] global that holds optimistic
//! thread, reaction and draft state). The mention and emoji typeahead in the composers
//! (`recur_view_widgets::suggest`) paint through the overlays `recur_view_chat::register` adds.
//!
//! **Observatory**: own a
//! [`surface::ReviewSurface`] entity built with [`surface::SurfaceProps`] (project, workspace,
//! base label, `active`, `tab`, `count`, findings by path, dirty paths, working dirty count,
//! exclude globs, `restore`). Push prop changes with `set_props`; bump `refresh` to reload.
//! Pass `onAddFinding` with `set_add_finding`. Subscribe to [`surface::SurfaceEvent`]:
//! `TabChange` (the host owns `tab`), `OpenFile`, `OpenFinding`, `ShowFindings`, and `Bound`
//! (commit count, commit scope, working open, totals and file count for the host's header).
//! `view_state` snapshots tab, selection and scroll for the host to persist; feed it back as
//! `restore`.
//!
//! **PR detail** (Files tab): there is no surface. Own a
//! [`stack::FileStack`] (stacked [`diff_file::DiffFile`] cards with windowed hydration, sticky
//! headers, scroll spy and j/k stepping) and a [`file_list::ReviewFileList`] entity. Feed the
//! stack `set_files` and [`stack::CardProps`] with `note_mode: Some(NoteMode::Pr)`, and call
//! `FileStack::render` from the host's render with a projection to the stack field. Wire the
//! list's select callback to `FileStack::select_path` and pass `comments_by_path` for its
//! unresolved-note flags. Line notes, threads, replies, resolve and reactions read the selected
//! PR from `AppState` and write through [`store`]; the commits tab uses [`commits::CommitsView`]
//! and [`commits::commit_row`].

pub mod autosize;
pub mod comment_header;
pub mod comment_hunk;
pub mod commit_meta;
pub mod commits;
pub mod diff_file;
pub mod diff_rows;
pub mod extra;
pub mod file_list;
pub mod files_changed;
pub mod findings;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod html;
pub mod line_note;
pub mod note_portal;
pub mod pane;
pub mod reactions;
pub mod review_diff;
pub mod review_scope;
pub mod review_scroll;
pub mod review_window;
pub mod scroll_fade;
#[cfg(test)]
mod spec;
pub mod split_diff;
pub mod spy_pick;
pub mod stack;
pub mod store;
pub mod style;
pub mod surface;
pub mod syntax;
pub mod tree_sticky;

use recur_view_core::ViewRegistry;

/// Installs the review store. The surface has no routes of its own: its hosts embed it.
pub fn register(r: &mut ViewRegistry) {
    r.init(|cx| cx.set_global(store::ReviewStore::default()));
}
