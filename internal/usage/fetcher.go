package usage

import (
	"context"
	"net/http"
	"sync"
	"time"
)

// Fetcher gathers one snapshot. Tests override the function fields.
type Fetcher struct {
	HTTP    *http.Client
	DataDir func() (string, error)
	Now     func() time.Time

	ClaudeToken func() (string, error)
	// ClaudeCreds and NudgeClaude are production-path hooks. Tests that
	// set ClaudeToken skip expiry handling. ClaudeCreds reads the Claude
	// Code keychain/files; NudgeClaude runs `claude auth status` so an
	// expired access token is rotated the same way Settings → Providers
	// already does.
	ClaudeCreds func() claudeOAuth
	NudgeClaude func(ctx context.Context)
	CodexAuth   func() (codexAuth, error)
	CursorToken func() (cursorTok, error)

	ClaudeUsageURL     string
	CodexUsageURL      string
	CodexMonthlyURL    string
	CursorPeriodURL    string
	CursorAggURL       string
	CursorProfileURL   string
	CursorMeURL        string
	CursorWebMeURL     string
	CursorGetMeURL     string
	CursorLoginURL     string
	CursorPollURL      string
	CursorRefreshURL   string
	CursorMintURL      string
	CursorListKeysURL  string
	CursorRevokeKeyURL string
	// OnMintedAPIKey is called when Connect (or a later fetch) mints or
	// rotates a user API key. prev is empty on first mint.
	OnMintedAPIKey func(prev, next string)

	WantClaude bool
	WantCodex  bool
	WantCursor bool
	// Want, when set, is consulted on every Collect so Settings flips apply
	// without restarting the poller. Bools above are the test fallback.
	Want func() (claude, codex, cursor bool)
	// CursorStub, when Cursor is not fetched, may append a local-only row
	// (Disconnect tokens, or a no-session Connect hint) with no network call.
	CursorStub func() *ProviderQuota

	mu            sync.Mutex
	connecting    bool
	connectCancel context.CancelFunc
	connectURL    string
}

func (f *Fetcher) client() *http.Client {
	if f.HTTP != nil {
		return f.HTTP
	}
	return defaultClient()
}

func (f *Fetcher) now() time.Time {
	if f.Now != nil {
		return f.Now()
	}
	return time.Now()
}

func (f *Fetcher) dataDir() string {
	if f.DataDir == nil {
		return ""
	}
	d, err := f.DataDir()
	if err != nil {
		return ""
	}
	return d
}

func (f *Fetcher) claudeCreds(ctx context.Context) claudeOAuth {
	if f.ClaudeCreds != nil {
		return f.ClaudeCreds()
	}
	return defaultClaudeCreds(ctx)
}

func (f *Fetcher) nudgeClaude(ctx context.Context) {
	if f.NudgeClaude != nil {
		f.NudgeClaude(ctx)
		return
	}
	defaultNudgeClaudeAuth(ctx)
}

// claudeAccess returns a usable Claude OAuth access token. Access tokens
// expire in hours; Claude Code itself refreshes them via `claude auth status`
// (Settings → Providers). Recur used to send the dead token and paint the
// usage meter as logged out. nudged is true when this call already ran
// `claude auth status`, so fetchClaude must not spawn a second one.
func (f *Fetcher) claudeAccess(ctx context.Context) (tok string, nudged bool, err error) {
	if f.ClaudeToken != nil {
		tok, err = f.ClaudeToken()
		return tok, false, err
	}
	creds := f.claudeCreds(ctx)
	if claudeAccessNeedsRefresh(creds, f.now()) {
		f.nudgeClaude(ctx)
		nudged = true
		if next := f.claudeCreds(ctx); next.Access != "" {
			creds = next
		}
	}
	if creds.Access == "" {
		return "", nudged, errNoClaudeToken
	}
	return creds.Access, nudged, nil
}

func (f *Fetcher) codexAuth() (codexAuth, error) {
	if f.CodexAuth != nil {
		return f.CodexAuth()
	}
	return defaultCodexAuth(f.dataDir())
}

func (f *Fetcher) cursorToken() (cursorTok, error) {
	if f.CursorToken != nil {
		return f.CursorToken()
	}
	return defaultCursorToken(f.dataDir())
}

func (f *Fetcher) claudeUsageURL() string {
	if f.ClaudeUsageURL != "" {
		return f.ClaudeUsageURL
	}
	return defaultClaudeUsageURL
}

func (f *Fetcher) codexUsageURL() string {
	if f.CodexUsageURL != "" {
		return f.CodexUsageURL
	}
	return defaultCodexUsageURL
}

func (f *Fetcher) codexMonthlyURL(accountID string) string {
	if f.CodexMonthlyURL != "" {
		return f.CodexMonthlyURL
	}
	return codexMonthlyUsageURL(f.codexUsageURL(), accountID)
}

func (f *Fetcher) cursorPeriodURL() string {
	if f.CursorPeriodURL != "" {
		return f.CursorPeriodURL
	}
	return defaultCursorPeriodURL
}

func (f *Fetcher) cursorAggURL() string {
	if f.CursorAggURL != "" {
		return f.CursorAggURL
	}
	return defaultCursorAggURL
}

func (f *Fetcher) cursorProfileURL() string {
	if f.CursorProfileURL != "" {
		return f.CursorProfileURL
	}
	return defaultCursorProfileURL
}

func (f *Fetcher) cursorMeURL() string {
	if f.CursorMeURL != "" {
		return f.CursorMeURL
	}
	return defaultCursorMeURL
}

func (f *Fetcher) cursorWebMeURL() string {
	if f.CursorWebMeURL != "" {
		return f.CursorWebMeURL
	}
	return defaultCursorWebMeURL
}

func (f *Fetcher) cursorGetMeURL() string {
	if f.CursorGetMeURL != "" {
		return f.CursorGetMeURL
	}
	return defaultCursorGetMeURL
}

func (f *Fetcher) cursorPollURL() string {
	if f.CursorPollURL != "" {
		return f.CursorPollURL
	}
	return defaultCursorPollURL
}

func (f *Fetcher) cursorRefreshURL() string {
	if f.CursorRefreshURL != "" {
		return f.CursorRefreshURL
	}
	return defaultCursorRefreshURL
}

func (f *Fetcher) cursorMintURL() string {
	if f.CursorMintURL != "" {
		return f.CursorMintURL
	}
	return defaultCursorMintURL
}

func (f *Fetcher) cursorListKeysURL() string {
	if f.CursorListKeysURL != "" {
		return f.CursorListKeysURL
	}
	return defaultCursorListKeysURL
}

func (f *Fetcher) cursorRevokeKeyURL() string {
	if f.CursorRevokeKeyURL != "" {
		return f.CursorRevokeKeyURL
	}
	return defaultCursorRevokeKeyURL
}

func (f *Fetcher) cursorConnecting() bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.connecting
}

func (f *Fetcher) maybeRefreshCursor(ctx context.Context, tok cursorTok) (cursorTok, bool) {
	if tok.Source != SourceCursorConnect || tok.Refresh == "" || !jwtNeedsRefresh(tok.Access) {
		return tok, false
	}
	next, err := refreshCursorToken(ctx, f, tok.Refresh)
	if err != nil || next.Access == "" {
		return tok, false
	}
	if next.Email == "" {
		next.Email = tok.Email
	}
	if next.APIKey == "" {
		next.APIKey = tok.APIKey
		next.APIKeyExpiresAtMs = tok.APIKeyExpiresAtMs
		next.APIKeyID = tok.APIKeyID
	}
	if dir := f.dataDir(); dir != "" {
		_ = writeCursorAuthFile(cursorAuthPath(dir), next)
	}
	return next, true
}

func (f *Fetcher) wants() (claude, codex, cursor bool) {
	if f.Want != nil {
		return f.Want()
	}
	return f.WantClaude, f.WantCodex, f.WantCursor
}

func claudeFetchCtx(ctx context.Context, others bool) (context.Context, context.CancelFunc) {
	if !others {
		return context.WithCancel(ctx)
	}
	return context.WithTimeout(ctx, claudeCollectBudget)
}

// Collect fetches every requested provider. A 429 on one provider is marked
// rate_limited so the poller can keep the last-known row.
func (f *Fetcher) Collect(ctx context.Context) Snapshot {
	snap := Snapshot{FetchedAt: f.now().UnixMilli(), Providers: []ProviderQuota{}}
	claude, codex, cursor := f.wants()
	if claude {
		cctx, cancel := claudeFetchCtx(ctx, codex || cursor)
		snap.Providers = append(snap.Providers, fetchClaude(cctx, f))
		cancel()
	}
	if codex {
		snap.Providers = append(snap.Providers, fetchCodex(ctx, f))
	}
	if cursor {
		snap.Providers = append(snap.Providers, fetchCursor(ctx, f))
	} else if f.cursorConnecting() {
		snap.Providers = append(snap.Providers, CursorConnectingQuota())
	} else if f.CursorStub != nil {
		if q := f.CursorStub(); q != nil {
			snap.Providers = append(snap.Providers, *q)
		}
	}
	return snap
}
