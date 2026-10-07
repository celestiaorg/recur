package bundles_test

import (
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestFormatReportColor(t *testing.T) {
	r := bundles.Report{
		Passed: 1,
		Failed: 1,
		Results: []bundles.CaseResult{
			{Bundle: "B", Name: "pass", OK: true},
			{Bundle: "B", Name: "fail", OK: false, Detail: "boom", File: "x.toml"},
		},
	}
	plain := bundles.FormatReport(r, false)
	if strings.Contains(plain, "\033[") {
		t.Fatalf("plain report should not contain ANSI: %q", plain)
	}
	colored := bundles.FormatReport(r, true)
	if !strings.Contains(colored, "\033[32m") {
		t.Fatalf("colored report missing green: %q", colored)
	}
	if !strings.Contains(colored, "\033[31m") {
		t.Fatalf("colored report missing red: %q", colored)
	}
	if !strings.Contains(colored, "ok") || !strings.Contains(colored, "FAIL") {
		t.Fatalf("colored report missing status tokens: %q", colored)
	}
}
