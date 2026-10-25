package app

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"strings"
	"time"
)

// ClaudeStatus is what the Settings screen renders for the Claude Code account.
type ClaudeStatus struct {
	// Authenticated reports whether `claude auth status` shows a logged-in user.
	Authenticated bool `json:"authenticated"`
	// Path is the resolved Claude Code binary on PATH ("" when not found).
	Path string `json:"path"`
	// Version is the Claude Code CLI version ("" when unknown).
	Version string `json:"version"`
	// Provider is a human-readable API provider label (e.g. "Anthropic API").
	Provider string `json:"provider"`
	// LoginMethod is a human-readable login/plan label (e.g. "Claude Team account").
	LoginMethod string `json:"loginMethod"`
	// Organization is the Claude org name when present.
	Organization string `json:"organization"`
	// Email is the logged-in account email when present.
	Email string `json:"email"`
	// Error is a short user-facing reason when not authenticated.
	Error string `json:"error"`
}

type claudeAuthStatusJSON struct {
	LoggedIn         bool   `json:"loggedIn"`
	AuthMethod       string `json:"authMethod"`
	APIProvider      string `json:"apiProvider"`
	Email            string `json:"email"`
	OrgName          string `json:"orgName"`
	SubscriptionType string `json:"subscriptionType"`
}

// claudeCmdTimeout bounds each Claude CLI probe. Version and auth status each
// get their own budget — sharing one deadline let a slow --version starve
// auth status and misreport a logged-in user as logged out.
const claudeCmdTimeout = 5 * time.Second

// GetClaudeStatus probes the local Claude Code CLI for login + version info.
// Safe to call from Settings: bounded, never invents a success state.
//
// `claude auth status` exits non-zero when logged out but still prints JSON —
// that must read as "Not logged in", not "not detected". "Claude Code not
// detected" is reserved for a missing binary.
func (s *SettingsService) GetClaudeStatus() ClaudeStatus {
	path := resolveClaudePath()

	vctx, vcancel := context.WithTimeout(context.Background(), claudeCmdTimeout)
	version := probeClaudeVersion(vctx)
	vcancel()

	actx, acancel := context.WithTimeout(context.Background(), claudeCmdTimeout)
	defer acancel()
	// Output() still returns stdout when the process exits non-zero (logged out).
	// On deadline, Output's error is typically *exec.ExitError ("signal: killed"),
	// not context.DeadlineExceeded — use actx.Err() for the timeout signal.
	out, err := exec.CommandContext(actx, "claude", "auth", "status").Output()
	return classifyClaudeStatus(path, version, out, err, actx.Err())
}

// classifyClaudeStatus turns path + version + auth-status bytes/err into a UI status.
// ctxErr is the probe context's Err() after the command returns (non-nil on timeout).
func classifyClaudeStatus(path, version string, out []byte, err, ctxErr error) ClaudeStatus {
	if st, ok := parseClaudeAuthStatus(out); ok {
		st.Path = path
		st.Version = version
		return st
	}
	if path == "" && version == "" && claudeBinaryMissing(err) {
		return ClaudeStatus{Error: "Claude Code not detected"}
	}
	if ctxErr != nil {
		return ClaudeStatus{
			Path:    path,
			Version: version,
			Error:   "Status check timed out",
		}
	}
	return ClaudeStatus{
		Path:    path,
		Version: version,
		Error:   "Not logged in",
	}
}

// resolveClaudePath returns the absolute path of `claude` on PATH, or "".
func resolveClaudePath() string {
	p, err := exec.LookPath("claude")
	if err != nil {
		return ""
	}
	return p
}

// claudeBinaryMissing reports whether err means `claude` is not on PATH.
func claudeBinaryMissing(err error) bool {
	if err == nil {
		return false
	}
	var pathErr *exec.Error
	if errors.As(err, &pathErr) && errors.Is(pathErr.Err, exec.ErrNotFound) {
		return true
	}
	return errors.Is(err, exec.ErrNotFound)
}

// DetectAgent reports the connected coding agent (PRD §16.13 first-run).
// It runs `claude auth status` to surface the logged-in account email/org so
// the welcome screen shows who is actually logged in. Returns "" when Claude is
// missing, not logged in, or the status call fails — never a fake success string
// that would paint Welcome step 1 green before a real login exists.
func (s *SettingsService) DetectAgent() string {
	st := s.GetClaudeStatus()
	if !st.Authenticated {
		return ""
	}
	if st.Organization != "" && st.Email != "" {
		return "Claude Code · " + st.Email + " (" + st.Organization + ")"
	}
	if st.Email != "" {
		return "Claude Code · " + st.Email
	}
	return "Claude Code"
}

func probeClaudeVersion(ctx context.Context) string {
	out, err := exec.CommandContext(ctx, "claude", "--version").Output()
	if err != nil {
		return ""
	}
	return parseClaudeVersion(string(out))
}

// claudeModelInfo is one entry of the models list Claude Code returns from the
// stream-json initialize handshake.
type claudeModelInfo struct {
	Value         string `json:"value"`
	ResolvedModel string `json:"resolvedModel"`
	DisplayName   string `json:"displayName"`
}

// probeClaudeModels asks Claude Code which models the logged-in account can
// use. Only the initialize handshake is sent, so no turn runs and no tokens
// are spent. Returns nil when claude is missing or the reply is unreadable.
func probeClaudeModels(ctx context.Context) []claudeModelInfo {
	cmd := exec.CommandContext(ctx, "claude", "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose")
	cmd.Dir = os.TempDir()
	cmd.Stdin = strings.NewReader(`{"type":"control_request","request_id":"models","request":{"subtype":"initialize"}}` + "\n")
	out, _ := cmd.Output()
	return parseClaudeInitModels(out)
}

func parseClaudeInitModels(out []byte) []claudeModelInfo {
	dec := json.NewDecoder(bytes.NewReader(out))
	for {
		var line struct {
			Type     string `json:"type"`
			Response struct {
				RequestID string `json:"request_id"`
				Response  struct {
					Models []claudeModelInfo `json:"models"`
				} `json:"response"`
			} `json:"response"`
		}
		if err := dec.Decode(&line); err != nil {
			return nil
		}
		if line.Type == "control_response" && line.Response.RequestID == "models" {
			return line.Response.Response.Models
		}
	}
}

func parseClaudeVersion(raw string) string {
	// Typical: "2.1.224 (Claude Code)" — keep the version token only.
	s := strings.TrimSpace(raw)
	if s == "" {
		return ""
	}
	if i := strings.IndexAny(s, " \t\n"); i >= 0 {
		return s[:i]
	}
	return s
}

func parseClaudeAuthStatus(out []byte) (ClaudeStatus, bool) {
	var info claudeAuthStatusJSON
	if err := json.Unmarshal(out, &info); err != nil || !info.LoggedIn {
		return ClaudeStatus{}, false
	}
	return ClaudeStatus{
		Authenticated: true,
		Provider:      formatClaudeProvider(info.APIProvider),
		LoginMethod:   formatClaudeLoginMethod(info.AuthMethod, info.SubscriptionType),
		Organization:  strings.TrimSpace(info.OrgName),
		Email:         strings.TrimSpace(info.Email),
	}, true
}

func formatClaudeProvider(apiProvider string) string {
	switch strings.ToLower(strings.TrimSpace(apiProvider)) {
	case "", "firstparty":
		return "Anthropic API"
	case "bedrock":
		return "Amazon Bedrock"
	case "vertex":
		return "Google Vertex AI"
	case "foundry":
		return "Microsoft Foundry"
	default:
		return apiProvider
	}
}

func formatClaudeLoginMethod(authMethod, subscriptionType string) string {
	sub := strings.ToLower(strings.TrimSpace(subscriptionType))
	switch sub {
	case "team":
		return "Claude Team account"
	case "enterprise":
		return "Claude Enterprise account"
	case "pro":
		return "Claude Pro account"
	case "max":
		return "Claude Max account"
	case "free":
		return "Claude Free account"
	}

	method := strings.ToLower(strings.TrimSpace(authMethod))
	switch method {
	case "claude.ai", "claude_ai", "claudeai":
		return "Claude.ai account"
	case "api_key", "apikey", "api-key":
		return "API key"
	case "":
		return ""
	default:
		return authMethod
	}
}
