package observe

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestProjectFindingsOntoDiff(t *testing.T) {
	files := []domain.FileDiff{{
		Path: "pkg/a.go",
		Lines: []domain.DiffLine{
			{New: "10", Type: "add", CodeHTML: "x"},
			{New: "11", Type: "add", CodeHTML: "y"},
		},
	}}
	findings := []domain.Finding{{
		File: "pkg/a.go", Line: 11, Severity: domain.SevHigh, Message: "race", Status: "open",
	}}
	if !ProjectFindingsOntoDiff(files, findings) {
		t.Fatal("expected change")
	}
	if files[0].Lines[1].ExtraHTML == "" || files[0].Lines[0].ExtraHTML != "" {
		t.Fatalf("flag misplaced: %#v", files[0].Lines)
	}
}

func TestProjectFindingsOntoDiffRange(t *testing.T) {
	files := []domain.FileDiff{{
		Path: "pkg/a.go",
		Lines: []domain.DiffLine{
			{New: "10", Type: "add", CodeHTML: "a"},
			{New: "11", Type: "add", CodeHTML: "b"},
			{New: "12", Type: "add", CodeHTML: "c"},
		},
	}}
	findings := []domain.Finding{{
		File: "pkg/a.go", Line: 10, EndLine: 11, Severity: domain.SevLow, Message: "span", Status: "open",
	}}
	if !ProjectFindingsOntoDiff(files, findings) {
		t.Fatal("expected change")
	}
	if files[0].Lines[0].ExtraHTML == "" || files[0].Lines[1].ExtraHTML == "" {
		t.Fatalf("range should flag both lines: %#v", files[0].Lines)
	}
	if files[0].Lines[2].ExtraHTML != "" {
		t.Fatalf("line 12 should be outside range: %#v", files[0].Lines[2])
	}
}

func TestCollectMetricsAndImpact(t *testing.T) {
	root := t.TempDir()
	// Line 3 is the func def — must be an "add" line for impact.
	src := "package p\n\nfunc UniqueWidgetName() {\n\tif true {\n\t\tfor i := 0; i < 3; i++ {\n\t\t\t_ = i\n\t\t}\n\t}\n}\n"
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	_ = os.WriteFile(filepath.Join(root, "a_test.go"), []byte("package p\nfunc TestUniqueWidgetName(t *testing.T) {}\n"), 0o644)

	m := CollectMetrics(root)
	if m.Files < 1 || m.LOC < 1 {
		t.Fatalf("metrics: %#v", m)
	}

	files := []domain.FileDiff{{
		Path: "a.go",
		Lines: []domain.DiffLine{
			{New: "3", Type: "add", CodeHTML: "func UniqueWidgetName() {"},
		},
	}}
	impact := ComputeImpact(t.Context(), root, files)
	if len(impact) == 0 {
		t.Fatal("expected impact items for UniqueWidgetName")
	}
	found := false
	for _, it := range impact {
		if it.Kind == "symbol" && it.Name == "UniqueWidgetName" {
			found = true
		}
	}
	if !found {
		t.Fatalf("missing symbol impact: %#v", impact)
	}
}

func TestCollectMetricsIncludesMajorLanguages(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "a.py"), []byte("def foo():\n    if True:\n        pass\n"), 0o644)
	_ = os.WriteFile(filepath.Join(root, "a.rs"), []byte("fn foo() { if true {} }\n"), 0o644)
	_ = os.WriteFile(filepath.Join(root, "A.java"), []byte("class A { void foo() { if (true) {} } }\n"), 0o644)
	m := CollectMetrics(root)
	if m.Files < 3 {
		t.Fatalf("CollectMetrics files=%d, want >= 3", m.Files)
	}
}

func TestCollectMetricsFindsDuplicateBlocks(t *testing.T) {
	root := t.TempDir()
	block := strings.Join([]string{
		"func prepareWidgetContext(cfg Config) error {",
		"\tif cfg.Timeout <= 0 {",
		"\t\tcfg.Timeout = defaultWidgetTimeout",
		"\t}",
		"\tclient := newWidgetClient(cfg)",
		"\tif err := client.Connect(); err != nil {",
		"\t\treturn fmt.Errorf(\"widget connect: %w\", err)",
		"\t}",
		"\treturn nil",
		"}",
	}, "\n")
	srcA := "package p\n\n" + block + "\n"
	srcB := "package p\n\n" + block + "\n\nfunc other() {}\n"
	if err := os.WriteFile(filepath.Join(root, "a.go"), []byte(srcA), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "b.go"), []byte(srcB), 0o644); err != nil {
		t.Fatal(err)
	}
	m := CollectMetrics(root)
	if m.Duplication <= 0 {
		t.Fatalf("CollectMetrics duplication=%v, want > 0", m.Duplication)
	}
	if len(m.Duplicates) == 0 {
		t.Fatal("CollectMetrics duplicates empty, want at least one clone")
	}
	found := false
	for _, d := range m.Duplicates {
		if len(d.Occurrences) >= 2 {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("CollectMetrics duplicates missing multi-occurrence clone: %#v", m.Duplicates)
	}
}

func TestAnalyzeDelta(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "a.go"), []byte("package p\nfunc A() {}\n"), 0o644)
	prev := AnalyzeRoot(t.Context(), "p", "", root, nil, nil, false)
	_ = os.WriteFile(filepath.Join(root, "b.go"), []byte("package p\nfunc B() {\n\tif true {}\n}\n"), 0o644)
	next := Analyze(t.Context(), AnalyzeOpts{
		ProjectID: "p", Root: root, Previous: &prev,
	})
	if next.Delta == nil {
		t.Fatal("expected delta")
	}
	if next.Delta.LOC <= 0 {
		t.Fatalf("expected positive LOC delta, got %#v", next.Delta)
	}
}
