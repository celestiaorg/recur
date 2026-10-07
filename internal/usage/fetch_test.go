package usage

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestFetchClaudeOK(t *testing.T) {
	t.Parallel()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sk-ant-test" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		if r.Header.Get("anthropic-beta") != "oauth-2025-04-20" {
			t.Errorf("anthropic-beta = %q", r.Header.Get("anthropic-beta"))
		}
		io.WriteString(w, `{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}`)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		ClaudeToken:    func() (string, error) { return "sk-ant-test", nil },
		WantClaude:     true,
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "" || len(q.Windows) != 2 {
		t.Fatalf("fetchClaude = %+v", q)
	}
}

func TestFetchClaudeNudgesExpiredAccessToken(t *testing.T) {
	t.Parallel()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sk-ant-fresh" {
			t.Errorf("Authorization = %q, want Bearer sk-ant-fresh", r.Header.Get("Authorization"))
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		io.WriteString(w, `{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}`)
	}))
	t.Cleanup(srv.Close)
	now := time.UnixMilli(1_000_000)
	creds := claudeOAuth{Access: "sk-ant-stale", ExpiresAt: now.UnixMilli() - 1}
	nudges := 0
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds:    func() claudeOAuth { return creds },
		NudgeClaude: func(context.Context) {
			nudges++
			creds = claudeOAuth{Access: "sk-ant-fresh", ExpiresAt: now.Add(time.Hour).UnixMilli()}
		},
	}
	q := fetchClaude(context.Background(), f)
	if nudges != 1 {
		t.Errorf("NudgeClaude calls = %d, want 1", nudges)
	}
	if q.Error != "" || len(q.Windows) != 2 {
		t.Fatalf("fetchClaude = %+v, want windows after nudge", q)
	}
}

func TestFetchClaudeRetriesAfter401(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	creds := claudeOAuth{Access: "sk-ant-stale", ExpiresAt: now.Add(time.Hour).UnixMilli()}
	var gotAuth []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotAuth = append(gotAuth, r.Header.Get("Authorization"))
		if r.Header.Get("Authorization") != "Bearer sk-ant-fresh" {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		io.WriteString(w, `{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}`)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds:    func() claudeOAuth { return creds },
		NudgeClaude: func(context.Context) {
			creds = claudeOAuth{Access: "sk-ant-fresh", ExpiresAt: now.Add(2 * time.Hour).UnixMilli()}
		},
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "" || len(q.Windows) != 2 {
		t.Fatalf("fetchClaude = %+v, want windows after 401 retry", q)
	}
	if len(gotAuth) != 2 || gotAuth[0] != "Bearer sk-ant-stale" || gotAuth[1] != "Bearer sk-ant-fresh" {
		t.Errorf("Authorization sequence = %v, want stale then fresh", gotAuth)
	}
}

func TestFetchClaudeDoesNotNudgeWhenLoggedOut(t *testing.T) {
	t.Parallel()
	nudges := 0
	f := &Fetcher{
		ClaudeCreds: func() claudeOAuth { return claudeOAuth{} },
		NudgeClaude: func(context.Context) { nudges++ },
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "Not signed in to Claude Code" {
		t.Errorf("fetchClaude.Error = %q, want Not signed in to Claude Code", q.Error)
	}
	if nudges != 0 {
		t.Errorf("NudgeClaude calls = %d, want 0 when no token is stored", nudges)
	}
}

func TestFetchClaudeTokenOverrideSkipsNudge(t *testing.T) {
	t.Parallel()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	}))
	t.Cleanup(srv.Close)
	nudges := 0
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		ClaudeToken:    func() (string, error) { return "tok", nil },
		NudgeClaude:    func(context.Context) { nudges++ },
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "Claude Code login expired" {
		t.Errorf("fetchClaude.Error = %q, want Claude Code login expired", q.Error)
	}
	if nudges != 0 {
		t.Errorf("NudgeClaude calls = %d, want 0 when ClaudeToken is set", nudges)
	}
}

func TestFetchClaudeDoesNotNudgeTwice(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	creds := claudeOAuth{Access: "sk-ant-stale", ExpiresAt: now.UnixMilli() - 1}
	nudges := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds:    func() claudeOAuth { return creds },
		NudgeClaude:    func(context.Context) { nudges++ },
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "Claude Code login expired" {
		t.Errorf("fetchClaude.Error = %q, want Claude Code login expired", q.Error)
	}
	if nudges != 1 {
		t.Errorf("NudgeClaude calls = %d, want 1 (expiry nudge, no 401 retry)", nudges)
	}
}

func TestFetchClaudeDoesNotRetry403(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	nudges := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusForbidden)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds: func() claudeOAuth {
			return claudeOAuth{Access: "tok", ExpiresAt: now.Add(time.Hour).UnixMilli()}
		},
		NudgeClaude: func(context.Context) { nudges++ },
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "Claude Code login expired" {
		t.Errorf("fetchClaude.Error = %q, want Claude Code login expired", q.Error)
	}
	if nudges != 0 {
		t.Errorf("NudgeClaude calls = %d, want 0 on 403", nudges)
	}
}

func TestClaudeStepCtxReservesHTTP(t *testing.T) {
	t.Parallel()
	parent, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	defer cancel()
	ctx, stop := claudeStepCtx(parent, 5*time.Second, 4*time.Second)
	defer stop()
	dl, ok := ctx.Deadline()
	parentDL, _ := parent.Deadline()
	if !ok {
		t.Fatal("claudeStepCtx() deadline missing")
	}
	left := time.Until(dl)
	parentLeft := time.Until(parentDL)
	if left > 5*time.Second+50*time.Millisecond {
		t.Errorf("claudeStepCtx() remaining = %v, want <= 5s cap", left)
	}
	if parentLeft-left < 3*time.Second {
		t.Errorf("claudeStepCtx() reserved %v of parent %v, want ~4s tail", parentLeft-left, parentLeft)
	}

	tight, tightCancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer tightCancel()
	step, stopStep := claudeStepCtx(tight, 5*time.Second, 4*time.Second)
	defer stopStep()
	if time.Until(deadlineOf(step)) > time.Second {
		t.Errorf("claudeStepCtx(3s parent, 4s reserve) remaining = %v, want ~0 so HTTP keeps the tail", time.Until(deadlineOf(step)))
	}
}

func deadlineOf(ctx context.Context) time.Time {
	dl, _ := ctx.Deadline()
	return dl
}

func TestFetchClaudeNudgesRefreshOnlyBlob(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	creds := claudeOAuth{Refresh: "r"}
	nudges := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sk-ant-fresh" {
			t.Errorf("Authorization = %q, want Bearer sk-ant-fresh", r.Header.Get("Authorization"))
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		io.WriteString(w, `{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}`)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds:    func() claudeOAuth { return creds },
		NudgeClaude: func(context.Context) {
			nudges++
			creds = claudeOAuth{Access: "sk-ant-fresh", Refresh: "r", ExpiresAt: now.Add(time.Hour).UnixMilli()}
		},
	}
	q := fetchClaude(context.Background(), f)
	if nudges != 1 {
		t.Errorf("NudgeClaude calls = %d, want 1 for refresh-only blob", nudges)
	}
	if q.Error != "" || len(q.Windows) != 2 {
		t.Fatalf("fetchClaude = %+v, want windows after refresh-only nudge", q)
	}
}

func TestFetchClaudeKeepsCredsWhenRereadEmpty(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sk-ant-stale" {
			t.Errorf("Authorization = %q, want Bearer sk-ant-stale", r.Header.Get("Authorization"))
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		io.WriteString(w, `{"five_hour":{"utilization":10},"seven_day":{"utilization":2}}`)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		Now:            func() time.Time { return now },
		ClaudeCreds: func() claudeOAuth {
			n++
			if n == 1 {
				return claudeOAuth{Access: "sk-ant-stale", ExpiresAt: now.UnixMilli() - 1}
			}
			return claudeOAuth{}
		},
		NudgeClaude: func(context.Context) {},
	}
	q := fetchClaude(context.Background(), f)
	if q.Error != "" || len(q.Windows) != 2 {
		t.Fatalf("fetchClaude = %+v, want windows from pre-nudge token", q)
	}
}

func TestClaudeFetchCtxCapsWhenOthersWanted(t *testing.T) {
	t.Parallel()
	parent, cancel := context.WithTimeout(context.Background(), collectTimeout)
	defer cancel()
	ctx, stop := claudeFetchCtx(parent, true)
	defer stop()
	dl, ok := ctx.Deadline()
	parentDL, _ := parent.Deadline()
	if !ok || !dl.Before(parentDL) {
		t.Errorf("claudeFetchCtx(others=true) deadline %v, want before parent %v", dl, parentDL)
	}
	solo, stopSolo := claudeFetchCtx(parent, false)
	defer stopSolo()
	soloDL, ok := solo.Deadline()
	if !ok || !soloDL.Equal(parentDL) {
		t.Errorf("claudeFetchCtx(others=false) deadline %v, want parent %v", soloDL, parentDL)
	}
}

func TestFetchCodexAPIKeyOnly(t *testing.T) {
	t.Parallel()
	f := &Fetcher{
		CodexAuth: func() (codexAuth, error) {
			return codexAuth{HasAPIKey: true}, nil
		},
	}
	q := fetchCodex(context.Background(), f)
	if q.Source != SourceCodexAPIKey {
		t.Errorf("Source = %q, want %q", q.Source, SourceCodexAPIKey)
	}
	if !strings.Contains(q.Error, "no plan quota") {
		t.Errorf("Error = %q", q.Error)
	}
}

func TestFetchCodexOK(t *testing.T) {
	t.Parallel()
	reqs := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		reqs++
		if r.Header.Get("Authorization") != "Bearer chat-tok" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		if r.Header.Get("ChatGPT-Account-Id") != "acc" {
			t.Errorf("account = %q", r.Header.Get("ChatGPT-Account-Id"))
		}
		io.WriteString(w, `{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":5}}}`)
	}))
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:          srv.Client(),
		CodexUsageURL: srv.URL,
		CodexAuth: func() (codexAuth, error) {
			return codexAuth{AccessToken: "chat-tok", AccountID: "acc", Email: "codex@example.com", HasChatGPT: true}, nil
		},
	}
	q := fetchCodex(context.Background(), f)
	if q.Error != "" || q.Plan != "Plus" || q.Email != "codex@example.com" || len(q.Windows) != 1 {
		t.Fatalf("fetchCodex = %+v", q)
	}
	if reqs != 1 {
		t.Errorf("fetchCodex(plus) HTTP requests = %d, want 1 (no monthly fallback without spend_control)", reqs)
	}
}

func TestFetchCodexMonthlyFallback(t *testing.T) {
	t.Parallel()
	mux := http.NewServeMux()
	mux.HandleFunc("/wham/usage", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer chat-tok" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		io.WriteString(w, `{"plan_type":"education","rate_limit":{"primary_window":{"used_percent":14},"secondary_window":{"used_percent":33}},"spend_control":{"reached":false,"individual_limit":null}}`)
	})
	mux.HandleFunc("/accounts/acc/spend-controls/current-user/monthly-usage", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer chat-tok" {
			t.Errorf("monthly Authorization = %q", r.Header.Get("Authorization"))
		}
		if r.Header.Get("ChatGPT-Account-Id") != "acc" {
			t.Errorf("monthly account = %q", r.Header.Get("ChatGPT-Account-Id"))
		}
		io.WriteString(w, `{"current_month_usage":297.4,"effective_monthly_limit":{"limit":1000,"enforcement_mode":"HARD_CAP"}}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:          srv.Client(),
		CodexUsageURL: srv.URL + "/wham/usage",
		CodexAuth: func() (codexAuth, error) {
			return codexAuth{AccessToken: "chat-tok", AccountID: "acc", HasChatGPT: true}, nil
		},
	}
	q := fetchCodex(context.Background(), f)
	if q.Error != "" {
		t.Fatalf("fetchCodex(monthly fallback) error = %q", q.Error)
	}
	var monthly *Window
	for i := range q.Windows {
		if q.Windows[i].ID == WindowBilling {
			monthly = &q.Windows[i]
		}
	}
	if monthly == nil {
		t.Fatalf("fetchCodex(monthly fallback) windows = %+v, want billing window", q.Windows)
	}
	if monthly.UsedPct != 29.74 || monthly.Detail != "297 of 1,000 credits" {
		t.Errorf("fetchCodex(monthly fallback) monthly = %+v, want UsedPct 29.74 detail 297 of 1,000 credits", monthly)
	}
}

func TestFetchCodexMonthlyFallbackUsesUsageAccountID(t *testing.T) {
	t.Parallel()
	mux := http.NewServeMux()
	mux.HandleFunc("/wham/usage", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"plan_type":"education","account_id":"acc","rate_limit":{"primary_window":{"used_percent":14}},"spend_control":{"reached":false,"individual_limit":null}}`)
	})
	mux.HandleFunc("/accounts/acc/spend-controls/current-user/monthly-usage", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("ChatGPT-Account-Id") != "acc" {
			t.Errorf("monthly account = %q, want acc from usage payload", r.Header.Get("ChatGPT-Account-Id"))
		}
		io.WriteString(w, `{"current_month_usage":100,"effective_monthly_limit":{"limit":1000,"enforcement_mode":"HARD_CAP"}}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:          srv.Client(),
		CodexUsageURL: srv.URL + "/wham/usage",
		CodexAuth: func() (codexAuth, error) {
			return codexAuth{AccessToken: "chat-tok", HasChatGPT: true}, nil
		},
	}
	q := fetchCodex(context.Background(), f)
	if q.Error != "" {
		t.Fatalf("fetchCodex(usage account_id) error = %q", q.Error)
	}
	if !hasWindow(q.Windows, WindowBilling) {
		t.Fatalf("fetchCodex(usage account_id) windows = %+v, want billing window", q.Windows)
	}
}

func TestResolveCodexAccountID(t *testing.T) {
	t.Parallel()
	if got := resolveCodexAccountID(codexAuth{AccountID: "from-auth"}, []byte(`{"account_id":"from-usage"}`)); got != "from-auth" {
		t.Errorf("resolveCodexAccountID(auth set) = %q, want from-auth", got)
	}
	if got := resolveCodexAccountID(codexAuth{}, []byte(`{"account_id":"from-usage"}`)); got != "from-usage" {
		t.Errorf("resolveCodexAccountID(usage) = %q, want from-usage", got)
	}
	tok := testJWT(t, map[string]any{
		"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "from-jwt"},
	})
	if got := resolveCodexAccountID(codexAuth{AccessToken: tok}, []byte(`{}`)); got != "from-jwt" {
		t.Errorf("resolveCodexAccountID(access jwt) = %q, want from-jwt", got)
	}
	idTok := testJWT(t, map[string]any{
		"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "from-id-token"},
	})
	if got := resolveCodexAccountID(codexAuth{IDToken: idTok}, []byte(`{}`)); got != "from-id-token" {
		t.Errorf("resolveCodexAccountID(id token) = %q, want from-id-token", got)
	}
}

func TestCodexMonthlyUsageURL(t *testing.T) {
	t.Parallel()
	got := codexMonthlyUsageURL(defaultCodexUsageURL, "acc")
	want := "https://chatgpt.com/backend-api/accounts/acc/spend-controls/current-user/monthly-usage"
	if got != want {
		t.Errorf("codexMonthlyUsageURL(%q, %q) = %q, want %q", defaultCodexUsageURL, "acc", got, want)
	}
	if got := codexMonthlyUsageURL(defaultCodexUsageURL, ""); got != "" {
		t.Errorf("codexMonthlyUsageURL(default, empty account) = %q, want empty", got)
	}
}

func TestFetchCursorPeriodAndModels(t *testing.T) {
	t.Parallel()
	mux := http.NewServeMux()
	mux.HandleFunc("/period", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer connect-sess" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		io.WriteString(w, `{"billingCycleStart":"2026-08-01T00:00:00Z","billingCycleEnd":"2026-09-01T00:00:00Z","planUsage":{"autoPercentUsed":40,"apiPercentUsed":0,"totalPercentUsed":21.9,"limit":2000,"totalSpend":800}}`)
	})
	mux.HandleFunc("/agg", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"aggregations":[{"modelIntent":"composer-2.5","inputTokens":100,"outputTokens":20,"totalCents":80,"tier":2}]}`)
	})
	mux.HandleFunc("/me", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"user":{"email":"connect@cursor.dev"}}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	f := &Fetcher{
		HTTP:             srv.Client(),
		CursorPeriodURL:  srv.URL + "/period",
		CursorAggURL:     srv.URL + "/agg",
		CursorProfileURL: srv.URL + "/missing",
		CursorMeURL:      srv.URL + "/me",
		CursorToken: func() (cursorTok, error) {
			return cursorTok{Access: "connect-sess", Source: SourceCursorConnect}, nil
		},
	}
	q := fetchCursor(context.Background(), f)
	if q.Error != "" {
		t.Fatalf("fetchCursor error = %q hint = %q", q.Error, q.Hint)
	}
	if q.Email != "connect@cursor.dev" || q.Source != SourceCursorConnect {
		t.Errorf("identity = %+v", q)
	}
	if len(q.Windows) != 1 || q.Windows[0].UsedPct != 40 || q.Windows[0].Detail != "" {
		t.Errorf("windows = %+v", q.Windows)
	}
	if len(q.Models) != 1 || q.Models[0].Tokens != 120 || q.Models[0].UsedPct != 40 || q.Models[0].Bucket != BucketCursor {
		t.Errorf("models = %+v", q.Models)
	}
}

func TestFetchCursorEmailFromDashboardMe(t *testing.T) {
	t.Parallel()
	mux := http.NewServeMux()
	mux.HandleFunc("/webme", func(w http.ResponseWriter, r *http.Request) {
		c := r.Header.Get("Cookie")
		if !strings.Contains(c, "WorkosCursorSessionToken=user_01abc::") {
			t.Errorf("Cookie = %q", c)
		}
		io.WriteString(w, `{"email":"connect@cursor.dev","sub":"user_01abc"}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	tok := testJWT(t, map[string]any{"sub": "user_01abc", "type": "session"})
	f := &Fetcher{HTTP: srv.Client(), CursorWebMeURL: srv.URL + "/webme"}
	if got := fetchCursorEmail(context.Background(), f, tok); got != "connect@cursor.dev" {
		t.Fatalf("email = %q", got)
	}
}

func TestPollerRefreshAndStale(t *testing.T) {
	t.Parallel()
	n := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n++
		if n == 1 {
			io.WriteString(w, `{"five_hour":{"utilization":11}}`)
			return
		}
		w.WriteHeader(http.StatusTooManyRequests)
		io.WriteString(w, `rate_limit_error`)
	}))
	t.Cleanup(srv.Close)
	now := time.Date(2026, 8, 20, 12, 0, 0, 0, time.UTC)
	f := &Fetcher{
		HTTP:           srv.Client(),
		ClaudeUsageURL: srv.URL,
		ClaudeToken:    func() (string, error) { return "tok", nil },
		WantClaude:     true,
		Now:            func() time.Time { return now },
	}
	var published int
	p := NewPoller(f, func(name string, data any) {
		if name != "provider.usage" {
			t.Errorf("event = %q", name)
		}
		published++
	})
	first := p.Refresh(context.Background())
	if first.Stale || first.Providers[0].Windows[0].UsedPct != 11 {
		t.Fatalf("first = %+v", first)
	}
	now = now.Add(time.Minute)
	second := p.Refresh(context.Background())
	if !second.Stale || second.Providers[0].Windows[0].UsedPct != 11 {
		t.Fatalf("second = %+v", second)
	}
	if published != 2 {
		t.Errorf("published = %d, want 2", published)
	}
}

func TestMintCursorUserAPIKey(t *testing.T) {
	t.Parallel()
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sess" {
			t.Errorf("Authorization = %q", r.Header.Get("Authorization"))
		}
		body, _ := io.ReadAll(r.Body)
		if !strings.Contains(string(body), `"name":"Recur"`) {
			t.Errorf("body = %s, want Recur key name", body)
		}
		io.WriteString(w, `{"apiKey":"crsr_minted"}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	f := &Fetcher{HTTP: srv.Client(), CursorMintURL: srv.URL, Now: func() time.Time {
		return time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	}}
	got, exp, err := mintCursorUserAPIKey(context.Background(), f, "sess")
	if err != nil {
		t.Fatalf("mintCursorUserAPIKey: %v", err)
	}
	if got != "crsr_minted" {
		t.Errorf("mintCursorUserAPIKey() = %q, want crsr_minted", got)
	}
	wantExp := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC).Add(cursorMintKeyTTL).UnixMilli()
	if exp != wantExp {
		t.Errorf("mintCursorUserAPIKey() exp = %d, want %d", exp, wantExp)
	}
}

func TestEnsureCursorAPIKeyMintsOnce(t *testing.T) {
	t.Parallel()
	n := 0
	mux := http.NewServeMux()
	mux.HandleFunc("/mint", func(w http.ResponseWriter, r *http.Request) {
		n++
		io.WriteString(w, `{"apiKey":"crsr_once"}`)
	})
	mux.HandleFunc("/list", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"apiKeys":[]}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	var notified string
	dir := t.TempDir()
	f := &Fetcher{
		HTTP:              srv.Client(),
		CursorMintURL:     srv.URL + "/mint",
		CursorListKeysURL: srv.URL + "/list",
		DataDir:           func() (string, error) { return dir, nil },
		OnMintedAPIKey:    func(prev, next string) { notified = next },
	}
	tok := f.ensureCursorAPIKey(context.Background(), cursorTok{Access: "sess", Source: SourceCursorConnect})
	if tok.APIKey != "crsr_once" {
		t.Errorf("APIKey = %q, want crsr_once", tok.APIKey)
	}
	if notified != "crsr_once" {
		t.Errorf("OnMintedAPIKey = %q, want crsr_once", notified)
	}
	if got := RecurCursorAPIKey(dir); got != "crsr_once" {
		t.Errorf("RecurCursorAPIKey = %q, want crsr_once", got)
	}
	tok = f.ensureCursorAPIKey(context.Background(), tok)
	if n != 1 {
		t.Errorf("mint calls = %d, want 1", n)
	}
}

func TestCursorAPIKeyNeedsRefresh(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	cases := []struct {
		name string
		tok  cursorTok
		want bool
	}{
		{"missing", cursorTok{}, true},
		{"no expiry yet", cursorTok{APIKey: "crsr_x"}, false},
		{"6d left", cursorTok{APIKey: "crsr_x", APIKeyExpiresAtMs: now.Add(6 * 24 * time.Hour).UnixMilli()}, true},
		{"8d left", cursorTok{APIKey: "crsr_x", APIKeyExpiresAtMs: now.Add(8 * 24 * time.Hour).UnixMilli()}, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			if got := cursorAPIKeyNeedsRefresh(tc.tok, now); got != tc.want {
				t.Errorf("cursorAPIKeyNeedsRefresh(%s) = %v, want %v", tc.name, got, tc.want)
			}
		})
	}
}

func TestEnsureCursorAPIKeyRotatesAndRevokes(t *testing.T) {
	t.Parallel()
	var minted, revoked int
	mux := http.NewServeMux()
	mux.HandleFunc("/mint", func(w http.ResponseWriter, r *http.Request) {
		minted++
		io.WriteString(w, `{"apiKey":"crsr_newkeyXXXX"}`)
	})
	mux.HandleFunc("/list", func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"apiKeys":[
			{"id":11,"name":"Recur","maskedKey":"crsr_...old1","createdAt":"1"},
			{"id":22,"name":"Recur","maskedKey":"crsr_...XXXX","createdAt":"2"}
		]}`)
	})
	mux.HandleFunc("/revoke", func(w http.ResponseWriter, r *http.Request) {
		revoked++
		body, _ := io.ReadAll(r.Body)
		if !strings.Contains(string(body), `"id":11`) {
			t.Errorf("revoke body = %s, want id 11", body)
		}
		io.WriteString(w, `{}`)
	})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	now := time.Date(2026, 8, 21, 12, 0, 0, 0, time.UTC)
	var prev, next string
	dir := t.TempDir()
	f := &Fetcher{
		HTTP:               srv.Client(),
		CursorMintURL:      srv.URL + "/mint",
		CursorListKeysURL:  srv.URL + "/list",
		CursorRevokeKeyURL: srv.URL + "/revoke",
		DataDir:            func() (string, error) { return dir, nil },
		Now:                func() time.Time { return now },
		OnMintedAPIKey:     func(p, n string) { prev, next = p, n },
	}
	tok := cursorTok{
		Access:            "sess",
		APIKey:            "crsr_oldkeyold1",
		APIKeyID:          11,
		APIKeyExpiresAtMs: now.Add(time.Hour).UnixMilli(),
		Source:            SourceCursorConnect,
	}
	got := f.ensureCursorAPIKey(context.Background(), tok)
	if got.APIKey != "crsr_newkeyXXXX" {
		t.Errorf("APIKey = %q, want crsr_newkeyXXXX", got.APIKey)
	}
	if prev != "crsr_oldkeyold1" || next != "crsr_newkeyXXXX" {
		t.Errorf("OnMintedAPIKey(%q, %q)", prev, next)
	}
	if minted != 1 {
		t.Errorf("mint calls = %d, want 1", minted)
	}
	if revoked != 1 {
		t.Errorf("revoke calls = %d, want 1", revoked)
	}
	if got.APIKeyID != 22 {
		t.Errorf("APIKeyID = %d, want 22 (new masked key)", got.APIKeyID)
	}
	got = f.ensureCursorAPIKey(context.Background(), got)
	if minted != 1 {
		t.Errorf("second ensure minted = %d, want 1", minted)
	}
}

func TestMatchesMaskedCursorKey(t *testing.T) {
	t.Parallel()
	if !matchesMaskedCursorKey("crsr_abcdefe5f6", "crsr_...e5f6") {
		t.Error("matchesMaskedCursorKey(crsr_abcdefe5f6, crsr_...e5f6) = false, want true")
	}
	if matchesMaskedCursorKey("crsr_abcdefe5f6", "crsr_...zzzz") {
		t.Error("matchesMaskedCursorKey(crsr_abcdefe5f6, crsr_...zzzz) = true, want false")
	}
}

func TestGenerateCursorPKCE(t *testing.T) {
	t.Parallel()
	a, err := generateCursorPKCE("https://cursor.com/loginDeepControl")
	if err != nil {
		t.Fatal(err)
	}
	b, err := generateCursorPKCE("https://cursor.com/loginDeepControl")
	if err != nil {
		t.Fatal(err)
	}
	if a.Verifier == b.Verifier || a.UUID == b.UUID {
		t.Fatal("PKCE should be unique per call")
	}
	if !strings.Contains(a.LoginURL, "redirectTarget=sdk") || !strings.Contains(a.LoginURL, "challenge=") {
		t.Errorf("LoginURL = %q", a.LoginURL)
	}
}

func TestCollectHonorsWantCallback(t *testing.T) {
	t.Parallel()
	calls := 0
	f := &Fetcher{
		WantClaude: true,
		WantCodex:  true,
		WantCursor: true,
		Want:       func() (bool, bool, bool) { return false, false, false },
		ClaudeToken: func() (string, error) {
			calls++
			return "", fmt.Errorf("no")
		},
		CodexAuth: func() (codexAuth, error) {
			calls++
			return codexAuth{}, fmt.Errorf("no")
		},
		CursorToken: func() (cursorTok, error) {
			calls++
			return cursorTok{}, fmt.Errorf("no")
		},
	}
	snap := f.Collect(context.Background())
	if calls != 0 {
		t.Fatalf("token lookups = %d, want 0 when Want disables all", calls)
	}
	if len(snap.Providers) != 0 {
		t.Fatalf("providers = %+v, want none", snap.Providers)
	}
}

func TestCollectKeepsConnectingOverStub(t *testing.T) {
	t.Parallel()
	f := &Fetcher{
		Want: func() (bool, bool, bool) { return false, false, false },
		CursorStub: func() *ProviderQuota {
			q := CursorNoSessionQuota()
			return &q
		},
	}
	f.connecting = true
	snap := f.Collect(context.Background())
	if len(snap.Providers) != 1 || !snap.Providers[0].Connecting {
		t.Fatalf("got %+v, want connecting row", snap.Providers)
	}
	if snap.Providers[0].Error != "" {
		t.Fatalf("Error = %q, stub must not clobber Connect", snap.Providers[0].Error)
	}
}

func TestCollectCursorStubWithoutFetch(t *testing.T) {
	t.Parallel()
	calls := 0
	f := &Fetcher{
		WantCursor: true,
		Want:       func() (bool, bool, bool) { return false, false, false },
		CursorStub: func() *ProviderQuota {
			q := CursorConnectTokensQuota()
			return &q
		},
		CursorToken: func() (cursorTok, error) {
			calls++
			return cursorTok{Access: "x"}, nil
		},
	}
	snap := f.Collect(context.Background())
	if calls != 0 {
		t.Fatalf("CursorToken calls = %d, want 0", calls)
	}
	if len(snap.Providers) != 1 || snap.Providers[0].SourceID != SourceIDCursorConnect {
		t.Fatalf("stub = %+v", snap.Providers)
	}
}

func TestCopySnapshotIndependence(t *testing.T) {
	t.Parallel()
	s := Snapshot{Providers: []ProviderQuota{{Windows: []Window{{UsedPct: 1}}}}}
	cp := copySnapshot(s)
	cp.Providers[0].Windows[0].UsedPct = 99
	if s.Providers[0].Windows[0].UsedPct != 1 {
		t.Fatal("copySnapshot aliased windows")
	}
}
