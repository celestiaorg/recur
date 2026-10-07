package app

import (
	"context"
	"errors"
	"os/exec"
	"testing"
)

func TestParseClaudeAuthStatus(t *testing.T) {
	raw := []byte(`{
		"loggedIn": true,
		"authMethod": "claude.ai",
		"apiProvider": "firstParty",
		"email": "dev@example.com",
		"orgName": "Acme",
		"subscriptionType": "team"
	}`)
	st, ok := parseClaudeAuthStatus(raw)
	if !ok {
		t.Fatal("expected authenticated status")
	}
	if !st.Authenticated {
		t.Fatal("Authenticated = false")
	}
	if st.Provider != "Anthropic API" {
		t.Fatalf("Provider = %q", st.Provider)
	}
	if st.LoginMethod != "Claude Team account" {
		t.Fatalf("LoginMethod = %q", st.LoginMethod)
	}
	if st.Organization != "Acme" || st.Email != "dev@example.com" {
		t.Fatalf("org/email = %q / %q", st.Organization, st.Email)
	}
}

func TestParseClaudeAuthStatusNotLoggedIn(t *testing.T) {
	// Real logged-out payload still includes provider/method; exit code is
	// non-zero but stdout is valid JSON with loggedIn:false.
	raw := []byte(`{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}`)
	if _, ok := parseClaudeAuthStatus(raw); ok {
		t.Fatal("expected not authenticated")
	}
	if _, ok := parseClaudeAuthStatus([]byte(`not-json`)); ok {
		t.Fatal("expected parse failure")
	}
}

func TestClaudeBinaryMissing(t *testing.T) {
	if !claudeBinaryMissing(&exec.Error{Name: "claude", Err: exec.ErrNotFound}) {
		t.Fatal("expected missing for exec.ErrNotFound")
	}
	if claudeBinaryMissing(errors.New("exit status 1")) {
		t.Fatal("logged-out exit must not look like missing binary")
	}
	if claudeBinaryMissing(nil) {
		t.Fatal("nil err is not missing")
	}
}

func TestClassifyClaudeStatusLoggedOutStillDetected(t *testing.T) {
	// Matches real `claude auth status` when logged out: exit 1 + JSON body.
	out := []byte(`{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}`)
	st := classifyClaudeStatus("/usr/local/bin/claude", "2.1.224", out, errors.New("exit status 1"), nil)
	if st.Authenticated {
		t.Fatal("expected not authenticated")
	}
	if st.Error != "Not logged in" {
		t.Fatalf("Error = %q, want Not logged in", st.Error)
	}
	if st.Version != "2.1.224" || st.Path != "/usr/local/bin/claude" {
		t.Fatalf("Version/Path = %q / %q", st.Version, st.Path)
	}
}

func TestClassifyClaudeStatusMissingBinary(t *testing.T) {
	st := classifyClaudeStatus("", "", nil, &exec.Error{Name: "claude", Err: exec.ErrNotFound}, nil)
	if st.Error != "Claude Code not detected" {
		t.Fatalf("Error = %q", st.Error)
	}
}

func TestClassifyClaudeStatusTimedOut(t *testing.T) {
	// Real CommandContext timeout: Output err is "signal: killed", timeout is ctx.Err().
	killErr := errors.New("signal: killed")
	st := classifyClaudeStatus("/usr/local/bin/claude", "2.1.224", nil, killErr, context.DeadlineExceeded)
	if st.Error != "Status check timed out" {
		t.Fatalf("Error = %q", st.Error)
	}
	if st.Authenticated {
		t.Fatal("expected not authenticated on timeout")
	}
}

func TestParseClaudeVersion(t *testing.T) {
	if got := parseClaudeVersion("2.1.224 (Claude Code)\n"); got != "2.1.224" {
		t.Fatalf("got %q", got)
	}
	if got := parseClaudeVersion("  3.0.0\n"); got != "3.0.0" {
		t.Fatalf("got %q", got)
	}
}

func TestFormatClaudeLoginMethodFallsBackToAuthMethod(t *testing.T) {
	if got := formatClaudeLoginMethod("claude.ai", ""); got != "Claude.ai account" {
		t.Fatalf("got %q", got)
	}
	if got := formatClaudeLoginMethod("api_key", ""); got != "API key" {
		t.Fatalf("got %q", got)
	}
}

func TestFormatClaudeProvider(t *testing.T) {
	cases := map[string]string{
		"firstParty": "Anthropic API",
		"bedrock":    "Amazon Bedrock",
		"vertex":     "Google Vertex AI",
		"custom":     "custom",
	}
	for in, want := range cases {
		if got := formatClaudeProvider(in); got != want {
			t.Fatalf("formatClaudeProvider(%q) = %q, want %q", in, got, want)
		}
	}
}
