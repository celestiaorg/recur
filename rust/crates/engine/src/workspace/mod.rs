//! Workspace lifecycle outside the loop walk: kitchen ids, create/allocate,
//! setup/teardown hooks, remove/delete, the main working copy, its remote,
//! live branch sync and the sidebar diff sync.

pub mod branch_sync;
pub mod create;
pub mod diff_sync;
pub mod hooks;
pub mod ids;
pub mod main_copy;
pub mod remote;
pub mod teardown;

#[cfg(test)]
mod tests;
