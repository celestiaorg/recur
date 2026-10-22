//! E5 tests: Go `github_prs_test.go`, `pr_test.go`, `github_thread_test.go`
//! and `github_reaction_test.go` (engine halves), `github_test.go` (engine
//! glue), `merge_message_test.go`, `open_pr_result_test.go`,
//! `rework_test.go`, `workspace_base_test.go`, `workspace_pr_test.go`.
//! Go's function seams (`listGitHubPRsFn`, `getGitHubPRSnapshot`, …) are
//! [`recur_forge::testing::FakeForge`] handlers.

mod support;
mod hoist;
mod linking;
mod send;
mod sync;
