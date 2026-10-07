package app

import (
	"os"
	"path/filepath"
	"testing"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestCodeAndObservatoryServices(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "main.go"), []byte("package main\nfunc main() {}\n"), 0o644)

	st := store.NewInMemory()
	p, err := st.AddProject(domain.Project{
		ID: "p-obs", Name: "Obs", RepoPath: root, BaseBranch: "main",
	})
	if err != nil {
		t.Fatal(err)
	}
	eng := engine.New(st)
	svc := NewServices(eng)

	tree, err := svc.Code.ListTree(t.Context(), p.ID, "", "")
	if err != nil || len(tree) == 0 {
		t.Fatalf("ListTree: %v %#v", err, tree)
	}
	fc, err := svc.Code.ReadFile(p.ID, "", "main.go")
	if err != nil || fc.Content == "" {
		t.Fatalf("ReadFile: %v %#v", err, fc)
	}
	out, err := svc.Code.Outline(p.ID, "", "main.go")
	if err != nil || len(out) == 0 {
		t.Fatalf("Outline: %v %#v", err, out)
	}

	ws, err := st.AddWorkspace(domain.Workspace{
		ID: "w-obs", ProjectID: p.ID, Goal: "Obs loop",
	})
	if err != nil {
		t.Fatal(err)
	}
	st.AddFinding(domain.Finding{
		ID: "f1", WorkspaceID: ws.ID, Severity: "medium",
		Message: "example", Status: "open",
	})
	list := svc.Observatory.ListFindings(ws.ID)
	if len(list) != 1 || list[0].ID != "f1" {
		t.Fatalf("ListFindings: %#v", list)
	}
}

func TestCreateFinding(t *testing.T) {
	st := store.NewInMemory()
	ws, err := st.AddWorkspace(domain.Workspace{ID: "w-note", ProjectID: "p", Goal: "g"})
	if err != nil {
		t.Fatal(err)
	}
	svc := NewServices(engine.New(st))

	got, err := svc.Observatory.CreateFinding(ws.ID, "src/main.go", 12, "  check the timeout  ", 0)
	if err != nil {
		t.Fatalf("CreateFinding: %v", err)
	}
	if got.File != "src/main.go" || got.Line != 12 || got.EndLine != 0 || got.Message != "check the timeout" {
		t.Errorf("CreateFinding = %+v, want file/line/trimmed message", got)
	}
	if got.Status != string(domain.FindingOpen) || got.Severity != domain.SevInfo || got.Source != domain.FindingSourceUser {
		t.Errorf("CreateFinding status/sev/source = %q/%q/%q", got.Status, got.Severity, got.Source)
	}
	if list := svc.Observatory.ListFindings(ws.ID); len(list) != 1 || list[0].ID != got.ID {
		t.Fatalf("ListFindings after create = %#v", list)
	}

	tests := []struct {
		name string
		ws   string
		file string
		line int
		msg  string
	}{
		{"empty message", ws.ID, "a.go", 1, "  "},
		{"empty file", ws.ID, "", 1, "x"},
		{"negative line", ws.ID, "a.go", -1, "x"},
		{"missing workspace", "nope", "a.go", 1, "x"},
	}
	for _, tt := range tests {
		if _, err := svc.Observatory.CreateFinding(tt.ws, tt.file, tt.line, tt.msg, 0); err == nil {
			t.Errorf("CreateFinding(%s) = nil, want error", tt.name)
		}
	}

	span, err := svc.Observatory.CreateFinding(ws.ID, "src/main.go", 10, "this span", 14)
	if err != nil {
		t.Fatalf("CreateFinding range: %v", err)
	}
	if span.Line != 10 || span.EndLine != 14 {
		t.Errorf("CreateFinding range = line %d end %d, want 10–14", span.Line, span.EndLine)
	}
	same, err := svc.Observatory.CreateFinding(ws.ID, "src/main.go", 8, "same line", 8)
	if err != nil {
		t.Fatalf("CreateFinding same-line: %v", err)
	}
	if same.EndLine != 0 {
		t.Errorf("CreateFinding same-line EndLine = %d, want 0", same.EndLine)
	}
	for _, tt := range []struct {
		name string
		line int
		end  int
	}{
		{"negative end", 1, -1},
		{"end before start", 10, 5},
		{"end without start", 0, 5},
	} {
		if _, err := svc.Observatory.CreateFinding(ws.ID, "a.go", tt.line, "x", tt.end); err == nil {
			t.Errorf("CreateFinding(%s) = nil, want error", tt.name)
		}
	}
}
