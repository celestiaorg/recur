//! The snapshot payload (Go `usage/types.go`), shared with providers through
//! `recur_agent::quota`.

pub use recur_agent::quota::*;

/// The bus event carrying a [`Snapshot`].
pub const PROVIDER_USAGE_EVENT: &str = "provider.usage";
