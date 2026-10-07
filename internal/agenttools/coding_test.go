package agenttools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestReadEditWriteSandbox(t *testing.T) {
	wt := t.TempDir()
	outside := t.TempDir()
	if err := os.WriteFile(filepath.Join(wt, "a.go"), []byte("hello world\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	reg := NewRegistry(CodingTools()...)
	read, _ := reg.Get("Read")
	got, err := read.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"a.go"}`)})
	if err != nil {
		t.Fatalf("Read: %v", err)
	}
	if !strings.Contains(got, "hello world") {
		t.Errorf("Read() = %q, want hello world", got)
	}
	if _, err := read.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"` + outside + `/x"}`)}); err == nil {
		t.Fatal("Read(escape) error = nil, want escape error")
	}

	edit, _ := reg.Get("Edit")
	if _, err := edit.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"a.go","old_string":"hello","new_string":"hi"}`)}); err != nil {
		t.Fatalf("Edit: %v", err)
	}
	raw, _ := os.ReadFile(filepath.Join(wt, "a.go"))
	if string(raw) != "hi world\n" {
		t.Errorf("after Edit content = %q, want hi world\\n", raw)
	}
	if err := os.WriteFile(filepath.Join(wt, "dup.txt"), []byte("aaa\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := edit.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"dup.txt","old_string":"a","new_string":"A"}`)}); err == nil {
		t.Fatal("Edit(non-unique) error = nil, want uniqueness error")
	}

	write, _ := reg.Get("Write")
	if _, err := write.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"b.txt","content":"new"}`)}); err != nil {
		t.Fatalf("Write: %v", err)
	}
	raw, _ = os.ReadFile(filepath.Join(wt, "b.txt"))
	if string(raw) != "new" {
		t.Errorf("Write content = %q, want new", raw)
	}
	if _, err := write.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"../nope.txt","content":"x"}`)}); err == nil {
		t.Fatal("Write(escape) error = nil, want escape error")
	}
	if _, err := write.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"wipe.go","contents":"x"}`)}); err == nil {
		t.Fatal("Write(typo contents) error = nil, want required content")
	}
	if _, err := write.Execute(Call{Worktree: wt, Args: []byte(`{"file_path":"empty.txt","content":""}`)}); err != nil {
		t.Fatalf("Write(explicit empty) error = %v, want nil", err)
	}
}

func TestGrepAndGlob(t *testing.T) {
	wt := t.TempDir()
	_ = os.WriteFile(filepath.Join(wt, "a.go"), []byte("func Alpha()\n"), 0o644)
	_ = os.MkdirAll(filepath.Join(wt, "sub"), 0o755)
	_ = os.WriteFile(filepath.Join(wt, "sub", "b.txt"), []byte("func Alpha()\n"), 0o644)
	reg := NewRegistry(CodingTools()...)
	grep, _ := reg.Get("Grep")
	got, err := grep.Execute(Call{Worktree: wt, Args: []byte(`{"pattern":"Alpha","glob":"*.go"}`)})
	if err != nil {
		t.Fatalf("Grep: %v", err)
	}
	if !strings.Contains(got, "a.go:1:func Alpha()") {
		t.Errorf("Grep() = %q, want a.go:1:func Alpha()", got)
	}
	glob, _ := reg.Get("Glob")
	got, err = glob.Execute(Call{Worktree: wt, Args: []byte(`{"pattern":"*.go"}`)})
	if err != nil {
		t.Fatalf("Glob: %v", err)
	}
	if !strings.Contains(got, "a.go") {
		t.Errorf("Glob() = %q, want a.go", got)
	}
}

func TestBashAndTodo(t *testing.T) {
	wt := t.TempDir()
	reg := NewRegistry(CodingTools()...)
	bash, _ := reg.Get("Bash")
	got, err := bash.Execute(Call{Worktree: wt, BypassPerms: true, Args: []byte(`{"command":"echo hi"}`)})
	if err != nil {
		t.Fatalf("Bash: %v", err)
	}
	if !strings.Contains(got, "hi") {
		t.Errorf("Bash() = %q, want hi", got)
	}

	if _, err := bash.Execute(Call{Worktree: wt, Permitter: denyPermits{}, Args: []byte(`{"command":"echo x"}`)}); err == nil {
		t.Fatal("Bash without bypass error = nil, want permission denied")
	}

	todo, _ := reg.Get("TodoWrite")
	store := &TodoStore{}
	got, err = todo.Execute(Call{SessionID: "s1", Todos: store, Args: []byte(`{"todos":[{"id":"1","content":"do","status":"pending"}]}`)})
	if err != nil {
		t.Fatalf("TodoWrite: %v", err)
	}
	if !strings.Contains(got, `"id":"1"`) {
		t.Errorf("TodoWrite() = %q, want id 1", got)
	}
}

func TestRegistryAllowlist(t *testing.T) {
	reg := NewRegistry(CodingTools()...)
	got := reg.List([]string{"Read", "mcp__recur__ask_user"})
	if len(got) != 1 || got[0].Name() != "Read" {
		t.Errorf("List(Read) = %v, want [Read]", names(got))
	}
	if AdvertisedName(got[0], []string{"Read"}) != "Read" {
		t.Errorf("AdvertisedName(Read) = %q, want Read", AdvertisedName(got[0], []string{"Read"}))
	}
}

type denyPermits struct{}

func (denyPermits) RequestPermission(context.Context, string, string, string, json.RawMessage) (bool, string, error) {
	return false, "no", nil
}

func names(ts []Tool) []string {
	out := make([]string, len(ts))
	for i, t := range ts {
		out[i] = t.Name()
	}
	return out
}
