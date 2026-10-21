package codex

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestSeedAuthPreservesNewerHomeCopy(t *testing.T) {
	dir := t.TempDir()
	from := filepath.Join(dir, "machine-auth.json")
	to := filepath.Join(dir, "home", "auth.json")
	if err := os.WriteFile(from, []byte(`{"old":true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		t.Fatal(err)
	}
	newer := []byte(`{"refreshed":true}`)
	if err := os.WriteFile(to, newer, 0o600); err != nil {
		t.Fatal(err)
	}
	// Make the destination clearly newer than the machine copy.
	past := time.Now().Add(-time.Hour)
	_ = os.Chtimes(from, past, past)
	if err := seedAuth(from, to); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(to)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != string(newer) {
		t.Fatalf("clobbered refreshed auth: %s", got)
	}
}

func TestSeedAuthCopiesWhenMissing(t *testing.T) {
	dir := t.TempDir()
	from := filepath.Join(dir, "machine-auth.json")
	to := filepath.Join(dir, "home", "auth.json")
	if err := os.WriteFile(from, []byte(`{"ok":true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := seedAuth(from, to); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(to)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != `{"ok":true}` {
		t.Fatalf("got %s", got)
	}
}

func TestSeedAuthUsesRecurLoginWithoutMachineLogin(t *testing.T) {
	dir := t.TempDir()
	to := filepath.Join(dir, "auth.json")
	if err := os.WriteFile(to, []byte(`{"tokens":{"access_token":"recur-login"}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := seedAuth(filepath.Join(dir, "missing-machine-auth.json"), to); err != nil {
		t.Fatalf("Recur-owned login required a machine login: %v", err)
	}
	if err := seedAuth(filepath.Join(dir, "missing-machine-auth.json"), filepath.Join(dir, "missing-auth.json")); err == nil {
		t.Fatal("missing both logins should still report an error")
	}
}

func TestEnsureTrustedProject(t *testing.T) {
	home := t.TempDir()
	wt := t.TempDir()
	if err := ensureTrustedProject(home, wt); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(filepath.Join(home, "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	s := string(raw)
	if !strings.Contains(s, "trust_level = \"trusted\"") {
		t.Fatalf("missing trust: %s", s)
	}
	if !strings.Contains(s, wt) {
		t.Fatalf("missing worktree path: %s", s)
	}
	// Idempotent.
	if err := ensureTrustedProject(home, wt); err != nil {
		t.Fatal(err)
	}
	raw2, _ := os.ReadFile(filepath.Join(home, "config.toml"))
	if strings.Count(string(raw2), "trust_level") != 1 {
		t.Fatalf("expected single trust entry, got:\n%s", raw2)
	}
}

func TestServerLaunchEnvStripsLinearAPIKey(t *testing.T) {
	got := serverLaunchEnv([]string{"PATH=/bin", "LINEAR_API_KEY=secret"}, "/tmp/codex-home", "sk-x")
	for _, e := range got {
		if strings.HasPrefix(e, "LINEAR_API_KEY=") {
			t.Fatalf("serverLaunchEnv still has %q", e)
		}
	}
	joined := strings.Join(got, "\n")
	if !strings.Contains(joined, "CODEX_HOME=/tmp/codex-home") {
		t.Fatalf("serverLaunchEnv missing CODEX_HOME: %v", got)
	}
	if !strings.Contains(joined, "CODEX_API_KEY=sk-x") {
		t.Fatalf("serverLaunchEnv missing CODEX_API_KEY: %v", got)
	}
	if !strings.Contains(joined, "PATH=/bin") {
		t.Fatalf("serverLaunchEnv dropped PATH: %v", got)
	}
}

func TestServerLaunchEnvUsesOnlyChosenAPIKey(t *testing.T) {
	base := []string{"PATH=/bin", "CODEX_API_KEY=ambient-codex", "OPENAI_API_KEY=ambient-openai"}
	for _, key := range []string{"", "chosen-key"} {
		env := serverLaunchEnv(base, "/tmp/codex-home", key)
		for _, entry := range env {
			if strings.HasPrefix(entry, "OPENAI_API_KEY=") ||
				(strings.HasPrefix(entry, "CODEX_API_KEY=") && (key == "" || entry != "CODEX_API_KEY="+key)) {
				t.Errorf("key %q: unexpected inherited credential %q", key, entry)
			}
		}
	}
}
