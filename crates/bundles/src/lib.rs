//! Recur Bundles: TOML-manifested packs that extend Recur with editor
//! commands (TypeScript or shebang macros), chat prompt actions, palette
//! search, New Task expanders, TextMate grammars and themes, terminal
//! themes, language data, snippets, symbols, language servers, settings,
//! and loop/role/policy definitions.
//!
//! Port of Go `internal/bundles`. Every user-visible message (parse errors,
//! skip reasons, lint findings, test details, report text) matches the Go
//! text byte for byte; [`go`] holds the Go stdlib semantics that requires.
//!
//! - Manifests: [`parse_manifest`], [`parse_manifest_full`] → [`ParsedBundle`].
//! - Discovery: [`discover_catalog`] over [`DiscoverRoots`] → [`Catalog`].
//! - Seeding: [`seed_defaults`] with [`SHIPPED_BUNDLES`] / [`SHIPPED_SKILLS`].
//! - TypeScript: [`compile_macro`], [`compile_prompt_macro`],
//!   [`compile_complete_macro`] (swc; see `compile.rs`).
//! - Shebang execution: [`run`], [`run_prompt`], [`run_prompt_complete_shebang`],
//!   [`collect_search_groups`], [`expand_matching`].
//! - Authoring: [`lint_bundles`], [`run_bundle_tests`], [`load_tests`].

pub mod go;

mod compile;
mod env;
mod exec;
mod grammar;
mod keyequiv;
mod langdata;
mod lint;
mod loops;
mod manifest;
mod prompt_run;
mod registry;
mod scope;
mod seed;
mod setting;
mod snippet;
mod symbol;
mod testcase;
mod testrun;
mod tomlx;

pub use compile::*;
pub use env::*;
pub use exec::*;
pub use grammar::*;
pub use keyequiv::*;
pub use langdata::*;
pub use lint::*;
pub use loops::*;
pub use manifest::*;
pub use prompt_run::*;
pub use registry::*;
pub use scope::*;
pub use seed::*;
pub use setting::*;
pub use snippet::*;
pub use symbol::*;
pub use testcase::*;
pub use testrun::*;

/// A bundles error. The text is the exact message Go produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

/// `Error(format!(…))`, the counterpart of Go's `fmt.Errorf`.
#[macro_export]
#[doc(hidden)]
macro_rules! err {
    ($($arg:tt)*) => {
        $crate::Error(format!($($arg)*))
    };
}
