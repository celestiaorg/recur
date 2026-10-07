package domain

import "testing"

func TestEffectiveRunsIgnoresEmpty(t *testing.T) {
	got := EffectiveRuns(Project{})
	if len(got) != 0 {
		t.Fatalf("EffectiveRuns(empty) = %+v, want empty", got)
	}
}

func TestEffectiveRunsExplicitWins(t *testing.T) {
	got := EffectiveRuns(Project{
		RunCommands: []RunCommand{
			{Name: "dev", Cmd: "wails3 task dev"},
			{Name: "test", Cmd: "go test ./..."},
			{Name: "  ", Cmd: ""},
			{Name: "", Cmd: "echo hi"},
		},
	})
	if len(got) != 3 {
		t.Fatalf("len=%d %+v", len(got), got)
	}
	if got[0].Name != "dev" || got[1].Name != "test" || got[2].Name != "run" {
		t.Fatalf("%+v", got)
	}
}

func TestDefaultRunPinned(t *testing.T) {
	p := Project{
		RunCommands: []RunCommand{{Name: "dev", Cmd: "a"}, {Name: "test", Cmd: "b"}},
		PinnedRun:   "test",
	}
	got := DefaultRun(p)
	if got.Name != "test" || got.Cmd != "b" {
		t.Fatalf("%+v", got)
	}
	p.PinnedRun = "missing"
	got = DefaultRun(p)
	if got.Name != "dev" {
		t.Fatalf("missing pin should fall back, got %+v", got)
	}
}

func TestVerifyDevCmdPrefersNamedDev(t *testing.T) {
	p := Project{
		RunCommands: []RunCommand{{Name: "test", Cmd: "go test"}, {Name: "dev", Cmd: "wails3 task dev"}},
		PinnedRun:   "test",
	}
	if got := VerifyDevCmd(p); got != "wails3 task dev" {
		t.Fatalf("verify should use named dev, got %q", got)
	}
	p.RunCommands = []RunCommand{{Name: "test", Cmd: "go test"}}
	if got := VerifyDevCmd(p); got != "go test" {
		t.Fatalf("no dev name → default, got %q", got)
	}
}

func TestUniquifyRunNames(t *testing.T) {
	got := EffectiveRuns(Project{RunCommands: []RunCommand{
		{Name: "dev", Cmd: "a"},
		{Name: "dev", Cmd: "b"},
	}})
	if got[0].Name != "dev" || got[1].Name != "dev-2" {
		t.Fatalf("%+v", got)
	}
}
