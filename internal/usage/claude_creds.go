package usage

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

const (
	claudeKeychainService = "Claude Code-credentials"
	// claudeAccessLeeway rotates before the usage API 401s. Access tokens
	// last hours, not days; two minutes is enough for one poller tick.
	claudeAccessLeeway = 2 * time.Minute
	claudeNudgeTimeout = 5 * time.Second
	// claudeHTTPReserve is left on the Claude ctx so doJSON still runs after
	// Keychain / `claude auth status`. Those sub-steps used to take the full
	// 5s cap each and spend claudeCollectBudget before the usage request.
	claudeHTTPReserve = 4 * time.Second
)

var errNoClaudeToken = fmt.Errorf("no Claude Code OAuth token on this machine")

// claudeOAuth is the Claude Code login blob (keychain on macOS, files elsewhere).
type claudeOAuth struct {
	Access    string
	Refresh   string
	ExpiresAt int64 // unix ms, 0 if unknown
}

func (c claudeOAuth) hasLogin() bool {
	return c.Access != "" || c.Refresh != ""
}

func defaultClaudeCreds(ctx context.Context) claudeOAuth {
	var keychain claudeOAuth
	if runtime.GOOS == "darwin" {
		keychain, _ = readClaudeKeychainCreds(ctx)
	}
	var file claudeOAuth
	for _, path := range claudeCredentialPaths() {
		raw, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		if c := parseClaudeOAuth(raw); c.hasLogin() {
			file = c
			break
		}
	}
	return pickClaudeCreds(keychain, file)
}

// claudeCredentialPaths is Keychain's file counterpart. CLAUDE_CONFIG_DIR
// (when set) is searched first so a leftover $HOME/.claude blob cannot
// shadow the live login.
func claudeCredentialPaths() []string {
	var out []string
	seen := map[string]bool{}
	add := func(p string) {
		if p == "" || seen[p] {
			return
		}
		seen[p] = true
		out = append(out, p)
	}
	if d := strings.TrimSpace(os.Getenv("CLAUDE_CONFIG_DIR")); d != "" {
		add(filepath.Join(d, ".credentials.json"))
		add(filepath.Join(d, "credentials.json"))
	}
	if home, err := os.UserHomeDir(); err == nil {
		add(filepath.Join(home, ".claude", ".credentials.json"))
		add(filepath.Join(home, ".claude", "credentials.json"))
		add(filepath.Join(home, ".claude.json"))
	}
	return out
}

// pickClaudeCreds prefers the blob with the later expiresAt so a leftover
// Keychain item cannot shadow a newer file login. When expiry is unknown,
// Keychain wins: that is the live macOS store.
func pickClaudeCreds(keychain, file claudeOAuth) claudeOAuth {
	switch {
	case !keychain.hasLogin() && !file.hasLogin():
		return claudeOAuth{}
	case !keychain.hasLogin():
		return file
	case !file.hasLogin():
		return keychain
	case keychain.ExpiresAt > 0 && file.ExpiresAt > 0 && file.ExpiresAt > keychain.ExpiresAt:
		return file
	case file.ExpiresAt > 0 && keychain.ExpiresAt == 0:
		return file
	default:
		return keychain
	}
}

func readClaudeKeychainCreds(ctx context.Context) (claudeOAuth, error) {
	cctx, cancel := claudeStepCtx(ctx, claudeNudgeTimeout, claudeHTTPReserve)
	defer cancel()
	out, err := exec.CommandContext(cctx, "security", "find-generic-password", "-s", claudeKeychainService, "-w").Output()
	if err != nil {
		return claudeOAuth{}, err
	}
	raw := strings.TrimSpace(string(out))
	if c := parseClaudeOAuth([]byte(raw)); c.hasLogin() {
		return c, nil
	}
	if looksLikeClaudeToken(raw) {
		return claudeOAuth{Access: raw}, nil
	}
	return claudeOAuth{}, fmt.Errorf("keychain item had no access token")
}

// defaultNudgeClaudeAuth asks the Claude Code CLI to rotate an expired
// access token. Recur does not consume the single-use refresh token itself
// (that races Claude Code and can wipe the Keychain login).
func defaultNudgeClaudeAuth(ctx context.Context) {
	cctx, cancel := claudeStepCtx(ctx, claudeNudgeTimeout, claudeHTTPReserve)
	defer cancel()
	_ = exec.CommandContext(cctx, "claude", "auth", "status").Run()
}

// claudeStepCtx caps a CLI/keychain step without spending the reserved tail
// of ctx, so the usage HTTP call is still reachable.
func claudeStepCtx(ctx context.Context, cap, reserve time.Duration) (context.Context, context.CancelFunc) {
	if ctx == nil {
		ctx = context.Background()
	}
	d := cap
	if dl, ok := ctx.Deadline(); ok {
		left := time.Until(dl) - reserve
		if left < d {
			d = left
		}
	}
	if d < 0 {
		d = 0
	}
	return context.WithTimeout(ctx, d)
}

func claudeAccessNeedsRefresh(c claudeOAuth, now time.Time) bool {
	if c.Access == "" {
		return c.Refresh != ""
	}
	if c.ExpiresAt <= 0 {
		return false
	}
	return !now.Add(claudeAccessLeeway).Before(time.UnixMilli(c.ExpiresAt))
}

func parseClaudeOAuth(raw []byte) claudeOAuth {
	s := strings.TrimSpace(string(raw))
	if looksLikeClaudeToken(s) {
		return claudeOAuth{Access: s}
	}
	m, err := decodeMap([]byte(s))
	if err != nil {
		return claudeOAuth{}
	}
	if c := claudeOAuthFromMap(m); c.Access != "" || c.Refresh != "" {
		return c
	}
	for _, k := range []string{"claudeAiOauth", "claudeAiOAuth", "oauth", "oauthAccount", "claude"} {
		if nested := mapOf(m[k]); nested != nil {
			if c := claudeOAuthFromMap(nested); c.Access != "" || c.Refresh != "" {
				return c
			}
		}
	}
	return claudeOAuth{}
}

func claudeOAuthFromMap(m map[string]any) claudeOAuth {
	c := claudeOAuth{
		Access:    firstString(m, "accessToken", "access_token", "token"),
		Refresh:   firstString(m, "refreshToken", "refresh_token"),
		ExpiresAt: parseResetsAt(firstAny(m, "expiresAt", "expires_at")),
	}
	if c.Access != "" || c.Refresh != "" {
		return c
	}
	if nested := mapOf(m["claudeAiOauth"]); nested != nil {
		return claudeOAuthFromMap(nested)
	}
	if nested := mapOf(m["claudeAiOAuth"]); nested != nil {
		return claudeOAuthFromMap(nested)
	}
	return c
}

func looksLikeClaudeToken(s string) bool {
	return strings.HasPrefix(strings.TrimSpace(s), "sk-ant-")
}

// defaultClaudeAccount reads the same local login identity Settings shows
// (`claude auth status` / ~/.claude.json), not the usage API.
func defaultClaudeAccount() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	for _, rel := range []string{".claude.json", filepath.Join(".claude", "config.json")} {
		raw, err := os.ReadFile(filepath.Join(home, rel))
		if err != nil {
			continue
		}
		if email := parseClaudeAccountFile(raw); email != "" {
			return email
		}
	}
	return ""
}

func parseClaudeAccountFile(raw []byte) string {
	m, err := decodeMap(raw)
	if err != nil {
		return ""
	}
	acc := mapOf(m["oauthAccount"])
	if acc == nil {
		acc = mapOf(m["oauth_account"])
	}
	if acc == nil {
		acc = m
	}
	return firstString(acc, "emailAddress", "email_address", "email", "userEmail")
}
