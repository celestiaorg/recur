use std::time::{SystemTime, UNIX_EPOCH};

use recur_domain::{AnalysisRun, Check, FileDiff, MetricDelta, TestResult};
use tokio_util::sync::CancellationToken;

use super::{collect_metrics, compute_impact, run_checks};

/// One observatory pass.
#[derive(Debug, Clone, Default)]
pub struct AnalyzeOpts {
    pub project_id: String,
    pub workspace_id: String,
    pub root: String,
    pub files: Vec<FileDiff>,
    pub test_result: Option<TestResult>,
    pub run_checks: bool,
    /// The prior run, for deltas.
    pub previous: Option<AnalysisRun>,
}

/// Metrics (and optionally checks) over `root`.
pub async fn analyze_root(
    cancel: &CancellationToken,
    project_id: &str,
    workspace_id: &str,
    root: &str,
    files: Vec<FileDiff>,
    test_result: Option<TestResult>,
    run_checks: bool,
) -> AnalysisRun {
    analyze(
        cancel,
        AnalyzeOpts {
            project_id: project_id.into(),
            workspace_id: workspace_id.into(),
            root: root.into(),
            files,
            test_result,
            run_checks,
            previous: None,
        },
    )
    .await
}

/// The full pass: metrics, impact for the diff, checks (or the Tester's
/// reported result), and the delta against `previous`.
pub async fn analyze(cancel: &CancellationToken, opts: AnalyzeOpts) -> AnalysisRun {
    let root = opts.root.clone();
    let metrics = tokio::task::spawn_blocking(move || collect_metrics(&root))
        .await
        .unwrap_or_default();
    let impact = if opts.files.is_empty() {
        Vec::new()
    } else {
        let (c, root, files) = (cancel.clone(), opts.root.clone(), opts.files.clone());
        tokio::task::spawn_blocking(move || compute_impact(&c, &root, &files))
            .await
            .unwrap_or_default()
    };
    let checks = if opts.run_checks {
        run_checks(Some(cancel.clone()), &opts.root, opts.test_result.as_ref()).await
    } else if let Some(tr) = &opts.test_result {
        let detail = match tr.summary.as_str() {
            "" if tr.passed => "Tester reported pass",
            "" => "Tester reported fail",
            s => s,
        };
        vec![Check {
            name: "Tests (agent)".into(),
            detail: detail.into(),
            passed: tr.passed,
            ..Default::default()
        }]
    } else {
        Vec::new()
    };
    let summary = format!(
        "{} files · {} LOC · {} hotspots · {} impact",
        metrics.files,
        metrics.loc,
        metrics.hotspots.len(),
        impact.len()
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let delta = opts.previous.as_ref().map(|prev| MetricDelta {
        loc: metrics.loc - prev.metrics.loc,
        max_complexity: metrics.max_complexity - prev.metrics.max_complexity,
        duplication: metrics.duplication - prev.metrics.duplication,
        todo_count: metrics.todo_count - prev.metrics.todo_count,
        hotspots: metrics.hotspots.len() as i64 - prev.metrics.hotspots.len() as i64,
    });
    AnalysisRun {
        id: format!("A-{}", now.as_nanos()),
        project_id: opts.project_id,
        workspace_id: opts.workspace_id,
        root: opts.root,
        when: "now".into(),
        ts: now.as_millis() as i64,
        metrics,
        impact,
        checks,
        summary,
        delta,
    }
}
