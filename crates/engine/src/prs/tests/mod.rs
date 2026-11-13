//! PR tests: linking, conversation, reactions, forge glue, merge messages,
//! open-PR results, rework, merge base and workspace landing. Forge calls are
//! [`recur_forge::testing::FakeForge`] handlers.

mod auto;
mod futures;
mod glue;
mod hoist;
mod linking;
mod merge;
mod rework;
mod send;
mod support;
mod sync;
mod thread;
mod workspace;
