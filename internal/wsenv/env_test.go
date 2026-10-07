package wsenv

import (
	"strings"
	"testing"

	"recur/internal/domain"
)

func TestParseDotenv(t *testing.T) {
	got := ParseDotenv(`
# comment
WAILS_VITE_PORT=$RECUR_PORT
export FOO=bar
BAZ="qux"
`)
	if got["WAILS_VITE_PORT"] != "$RECUR_PORT" || got["FOO"] != "bar" || got["BAZ"] != "qux" {
		t.Fatalf("%#v", got)
	}
}

func TestEnvironInjectsRecurAndProjectEnv(t *testing.T) {
	v := Vars{
		Port:         9246,
		WorkspaceID:  "LOOP-1",
		WorktreePath: "/tmp/wt",
		ProjectID:    "recur",
		Branch:       "mycodecrafting/smoky-shortcake",
		ProjectEnv: map[string]string{
			"WAILS_VITE_PORT":      "$RECUR_PORT",
			"RECUR_DATA_DIR":       "/tmp/data-$RECUR_WORKSPACE_ID",
			"RECUR_INSTANCE_LABEL": "$RECUR_BRANCH",
		},
	}
	env := v.Environ([]string{"PATH=/bin", "RECUR_PORT=1"})
	got := map[string]string{}
	for _, kv := range env {
		if i := strings.IndexByte(kv, '='); i > 0 {
			got[kv[:i]] = kv[i+1:]
		}
	}
	if got["RECUR_PORT"] != "9246" || got["PORT"] != "9246" {
		t.Errorf("port: RECUR_PORT=%q PORT=%q", got["RECUR_PORT"], got["PORT"])
	}
	if got["RECUR_WORKSPACE_ID"] != "LOOP-1" || got["RECUR_WORKTREE"] != "/tmp/wt" {
		t.Errorf("ids/path: %#v", got)
	}
	if got["RECUR_BRANCH"] != "mycodecrafting/smoky-shortcake" {
		t.Errorf("RECUR_BRANCH=%q", got["RECUR_BRANCH"])
	}
	if got["RECUR_INSTANCE_LABEL"] != "mycodecrafting/smoky-shortcake" {
		t.Errorf("RECUR_INSTANCE_LABEL=%q", got["RECUR_INSTANCE_LABEL"])
	}
	if got["WAILS_VITE_PORT"] != "9246" {
		t.Errorf("WAILS_VITE_PORT=%q", got["WAILS_VITE_PORT"])
	}
	if got["RECUR_DATA_DIR"] != "/tmp/data-LOOP-1" {
		t.Errorf("RECUR_DATA_DIR=%q", got["RECUR_DATA_DIR"])
	}
}

func TestExpandKnownLeavesUnknown(t *testing.T) {
	got := ExpandKnown("x=$RECUR_PORT y=$NOPE", map[string]string{"RECUR_PORT": "9"})
	if got != "x=9 y=$NOPE" {
		t.Errorf("got %q", got)
	}
}

func TestExpandKnownUsesHome(t *testing.T) {
	t.Setenv("HOME", "/Users/test")
	got := ExpandKnown("$HOME/Library/Recur-$RECUR_WORKSPACE_ID", map[string]string{
		"RECUR_WORKSPACE_ID": "W1",
	})
	if got != "/Users/test/Library/Recur-W1" {
		t.Errorf("got %q", got)
	}
}

func TestFromWorkspace(t *testing.T) {
	v := FromWorkspace(
		domain.Workspace{ID: "W1", ProjectID: "P1", WorktreePath: "/wt", Port: 3, Branch: " feat/x "},
		domain.Project{RepoPath: "/repo", Env: "A=1\n# hi\nB=two"},
	)
	if v.Port != 3 || v.WorkspaceID != "W1" || v.RepoPath != "/repo" || v.Branch != "feat/x" || v.ProjectEnv["A"] != "1" || v.ProjectEnv["B"] != "two" {
		t.Fatalf("%+v", v)
	}
}

func TestRecurMapIncludesEmptyBranch(t *testing.T) {
	m := Vars{WorkspaceID: "W1", ProjectID: "P1", WorktreePath: "/wt"}.RecurMap()
	if got, ok := m["RECUR_BRANCH"]; !ok || got != "" {
		t.Errorf("RecurMap() RECUR_BRANCH = %q ok=%v, want empty string", got, ok)
	}
}

func TestEnvironEmptyBranchHidesInstanceLabel(t *testing.T) {
	v := Vars{
		WorkspaceID: "W1",
		ProjectEnv:  map[string]string{"RECUR_INSTANCE_LABEL": "$RECUR_BRANCH"},
	}
	got := envMap(v.Environ([]string{"PATH=/bin"}))
	if got["RECUR_INSTANCE_LABEL"] != "" {
		t.Errorf("RECUR_INSTANCE_LABEL=%q, want empty", got["RECUR_INSTANCE_LABEL"])
	}
	if got["RECUR_BRANCH"] != "" {
		t.Errorf("RECUR_BRANCH=%q, want empty", got["RECUR_BRANCH"])
	}
}

func TestEnvironBranchDollarIsNotReexpanded(t *testing.T) {
	t.Setenv("HOME", "/Users/test")
	v := Vars{
		Branch:     "feat/$HOME",
		ProjectEnv: map[string]string{"RECUR_INSTANCE_LABEL": "$RECUR_BRANCH", "COPY": "$RECUR_INSTANCE_LABEL"},
	}
	got := envMap(v.Environ([]string{"PATH=/bin"}))
	if got["RECUR_INSTANCE_LABEL"] != "feat/$HOME" {
		t.Errorf("RECUR_INSTANCE_LABEL=%q, want %q", got["RECUR_INSTANCE_LABEL"], "feat/$HOME")
	}
	if got["COPY"] != "feat/$HOME" {
		t.Errorf("COPY=%q, want %q", got["COPY"], "feat/$HOME")
	}
	if got["RECUR_BRANCH"] != "feat/$HOME" {
		t.Errorf("RECUR_BRANCH=%q, want %q", got["RECUR_BRANCH"], "feat/$HOME")
	}
}

func TestFromWorkspaceStripsTomlPATH(t *testing.T) {
	v := FromWorkspace(
		domain.Workspace{ProjectID: "p", Port: 9},
		domain.Project{
			ID:             "p",
			RepoPath:       "/repo",
			HasProjectFile: true,
			Env:            "PATH=/tmp/evil\nOK=1\n",
		},
	)
	if v.Port != 9 || v.ProjectID != "p" || v.RepoPath != "/repo" {
		t.Fatalf("FromWorkspace fields = %+v", v)
	}
	if got, ok := v.ProjectEnv["PATH"]; ok {
		t.Errorf("FromWorkspace ProjectEnv PATH = %q, want absent", got)
	}
	if v.ProjectEnv["OK"] != "1" {
		t.Errorf("FromWorkspace ProjectEnv OK = %q, want 1", v.ProjectEnv["OK"])
	}
}
