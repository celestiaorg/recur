package observe

import (
	"context"
	"fmt"
	"time"

	"recur/internal/domain"
)

// AnalyzeOpts configures an observatory pass.
type AnalyzeOpts struct {
	ProjectID   string
	WorkspaceID string
	Root        string
	Files       []domain.FileDiff
	TestResult  *domain.TestResult
	RunChecks   bool
	Previous    *domain.AnalysisRun // for deltas
}

// AnalyzeRoot runs metrics (+ optional checks) over root and returns an AnalysisRun.
func AnalyzeRoot(ctx context.Context, projectID, workspaceID, root string, files []domain.FileDiff, testResult *domain.TestResult, runChecks bool) domain.AnalysisRun {
	return Analyze(ctx, AnalyzeOpts{
		ProjectID: projectID, WorkspaceID: workspaceID, Root: root,
		Files: files, TestResult: testResult, RunChecks: runChecks,
	})
}

// Analyze is the full observatory pass with optional prior snapshot.
func Analyze(ctx context.Context, opts AnalyzeOpts) domain.AnalysisRun {
	metrics := CollectMetrics(opts.Root)
	var impact []domain.ImpactItem
	if len(opts.Files) > 0 {
		impact = ComputeImpact(ctx, opts.Root, opts.Files)
	}
	var checks []domain.Check
	if opts.RunChecks {
		checks = RunChecks(ctx, opts.Root, opts.TestResult)
	} else if opts.TestResult != nil {
		detail := opts.TestResult.Summary
		if detail == "" {
			if opts.TestResult.Passed {
				detail = "Tester reported pass"
			} else {
				detail = "Tester reported fail"
			}
		}
		checks = []domain.Check{{Name: "Tests (agent)", Detail: detail, Passed: opts.TestResult.Passed}}
	}

	summary := fmt.Sprintf("%d files · %d LOC · %d hotspots · %d impact",
		metrics.Files, metrics.LOC, len(metrics.Hotspots), len(impact))

	run := domain.AnalysisRun{
		ID:          fmt.Sprintf("A-%d", time.Now().UnixNano()),
		ProjectID:   opts.ProjectID,
		WorkspaceID: opts.WorkspaceID,
		Root:        opts.Root,
		When:        "now",
		TS:          time.Now().UnixMilli(),
		Metrics:     metrics,
		Impact:      impact,
		Checks:      checks,
		Summary:     summary,
	}
	if opts.Previous != nil {
		run.Delta = &domain.MetricDelta{
			LOC:           metrics.LOC - opts.Previous.Metrics.LOC,
			MaxComplexity: metrics.MaxComplexity - opts.Previous.Metrics.MaxComplexity,
			Duplication:   metrics.Duplication - opts.Previous.Metrics.Duplication,
			TodoCount:     metrics.TodoCount - opts.Previous.Metrics.TodoCount,
			Hotspots:      len(metrics.Hotspots) - len(opts.Previous.Metrics.Hotspots),
		}
	}
	return run
}
