//! The code observatory: metrics, light checks, impact, and projecting
//! reviewer findings onto diffs (Go `internal/observe`).

mod analyze;
mod checks;
mod findings;
mod impact;
mod metrics;

pub use analyze::{AnalyzeOpts, analyze, analyze_root};
pub use checks::{run_checks, run_light_checks};
pub use findings::{finding_flag_html, project_findings_onto_diff};
pub use impact::compute_impact;
pub use metrics::collect_metrics;

/// Leading decimal digits of a diff line number (`""` and junk are 0).
pub(crate) fn parse_line(s: &str) -> i64 {
    let mut n: i64 = 0;
    for c in s.trim().bytes() {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.wrapping_mul(10).wrapping_add(i64::from(c - b'0'));
    }
    n
}

pub(crate) fn to_slash(p: &str) -> String {
    p.replace('\\', "/")
}

#[cfg(test)]
mod tests;
