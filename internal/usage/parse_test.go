package usage

import (
	"encoding/base64"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestParseClaudeCredentials(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		raw  string
		want string
	}{
		{"bare token", "sk-ant-oat01-abc", "sk-ant-oat01-abc"},
		{"oauth blob", `{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xyz","refreshToken":"r"}}`, "sk-ant-oat01-xyz"},
		{"snake case", `{"access_token":"sk-ant-oat01-sn"}`, "sk-ant-oat01-sn"},
		{"garbage", `not-json`, ""},
		{"empty", `{}`, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := parseClaudeOAuth([]byte(tc.raw)).Access
			if got != tc.want {
				t.Errorf("parseClaudeOAuth(%q).Access = %q, want %q", tc.raw, got, tc.want)
			}
		})
	}
}

func TestParseClaudeOAuthExpiresAt(t *testing.T) {
	t.Parallel()
	raw := []byte(`{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xyz","refreshToken":"sk-ant-ort01-r","expiresAt":1787565508430}}`)
	got := parseClaudeOAuth(raw)
	if got.Access != "sk-ant-oat01-xyz" || got.Refresh != "sk-ant-ort01-r" {
		t.Errorf("parseClaudeOAuth(oauth blob) = %+v, want access+refresh", got)
	}
	if got.ExpiresAt != 1787565508430 {
		t.Errorf("parseClaudeOAuth(oauth blob).ExpiresAt = %d, want 1787565508430", got.ExpiresAt)
	}
}

func TestPickClaudeCreds(t *testing.T) {
	t.Parallel()
	olderKC := claudeOAuth{Access: "kc", ExpiresAt: 1_000}
	newerFile := claudeOAuth{Access: "file", ExpiresAt: 2_000}
	newerKC := claudeOAuth{Access: "kc", ExpiresAt: 2_000}
	olderFile := claudeOAuth{Access: "file", ExpiresAt: 1_000}
	refreshOnly := claudeOAuth{Refresh: "r"}
	cases := []struct {
		name     string
		keychain claudeOAuth
		file     claudeOAuth
		want     string
	}{
		{"file newer expiresAt", olderKC, newerFile, "file"},
		{"keychain newer expiresAt", newerKC, olderFile, "kc"},
		{"unknown expiry prefers keychain", claudeOAuth{Access: "kc"}, claudeOAuth{Access: "file"}, "kc"},
		{"file known expiry beats unknown keychain", claudeOAuth{Access: "kc"}, newerFile, "file"},
		{"file only", claudeOAuth{}, newerFile, "file"},
		{"keychain only", olderKC, claudeOAuth{}, "kc"},
		{"refresh-only file", claudeOAuth{}, refreshOnly, "r"},
		{"empty", claudeOAuth{}, claudeOAuth{}, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := pickClaudeCreds(tc.keychain, tc.file)
			have := got.Access
			if have == "" {
				have = got.Refresh
			}
			if have != tc.want {
				t.Errorf("pickClaudeCreds(%s) = %+v, want %q", tc.name, got, tc.want)
			}
		})
	}
}

func TestClaudeCredentialPathsHonorsConfigDir(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("CLAUDE_CONFIG_DIR", dir)
	paths := claudeCredentialPaths()
	want := filepath.Join(dir, ".credentials.json")
	if len(paths) == 0 || paths[0] != want {
		t.Errorf("claudeCredentialPaths()[0] = %v, want %q first", paths, want)
	}
}

func TestClaudeAccessNeedsRefresh(t *testing.T) {
	t.Parallel()
	now := time.UnixMilli(1_000_000)
	cases := []struct {
		name string
		c    claudeOAuth
		want bool
	}{
		{"missing access", claudeOAuth{ExpiresAt: 1}, false},
		{"unknown expiry", claudeOAuth{Access: "tok"}, false},
		{"expired", claudeOAuth{Access: "tok", ExpiresAt: now.UnixMilli() - 1}, true},
		{"inside leeway", claudeOAuth{Access: "tok", ExpiresAt: now.Add(time.Minute).UnixMilli()}, true},
		{"fresh", claudeOAuth{Access: "tok", ExpiresAt: now.Add(time.Hour).UnixMilli()}, false},
		{"refresh only", claudeOAuth{Refresh: "r"}, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := claudeAccessNeedsRefresh(tc.c, now)
			if got != tc.want {
				t.Errorf("claudeAccessNeedsRefresh(%s) = %v, want %v", tc.name, got, tc.want)
			}
		})
	}
}

func TestParseClaudeUsage(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 78.5, "resets_at": "2026-08-21T01:00:00Z"},
		"seven_day": {"used_percentage": 13, "resets_at": 1775808000},
		"seven_day_opus": {"used_percent": 4.2},
		"extra_usage": {"is_enabled": true, "used_credits": 10000, "monthly_limit": 10000},
		"limits": [
			{"kind": "weekly_scoped", "percent": 54, "resets_at": "2026-08-28T00:00:00Z",
			 "scope": {"model": {"display_name": "Fable"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	if q.ID != ProviderClaude {
		t.Errorf("ID = %q, want %q", q.ID, ProviderClaude)
	}
	if !hasWindow(q.Windows, WindowFiveHour) || !hasWindow(q.Windows, WindowFable) || !hasWindow(q.Windows, WindowExtra) {
		t.Fatalf("windows = %+v", q.Windows)
	}
	if q.Windows[0].UsedPct != 78.5 {
		t.Errorf("five_hour UsedPct = %v, want 78.5", q.Windows[0].UsedPct)
	}
	wantReset := time.Date(2026, 8, 21, 1, 0, 0, 0, time.UTC).UnixMilli()
	if q.Windows[0].ResetsAt != wantReset {
		t.Errorf("five_hour ResetsAt = %d, want %d", q.Windows[0].ResetsAt, wantReset)
	}
	if q.Windows[1].UsedPct != 13 {
		t.Errorf("seven_day UsedPct = %v, want 13", q.Windows[1].UsedPct)
	}
	var fable, extra *Window
	for i := range q.Windows {
		switch q.Windows[i].ID {
		case WindowFable:
			fable = &q.Windows[i]
		case WindowExtra:
			extra = &q.Windows[i]
		}
	}
	if fable == nil || fable.UsedPct != 54 || fable.Label != "Fable (7-day)" {
		t.Errorf("fable = %+v", fable)
	}
	if extra == nil || extra.UsedPct != 100 || extra.Detail != "$100.00 of $100.00" {
		t.Errorf("extra = %+v", extra)
	}
}

func TestParseClaudeUsageInactiveFableLimit(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 10},
		"seven_day": {"utilization": 20},
		"limits": [
			{"kind": "weekly_scoped", "percent": 41, "is_active": false,
			 "scope": {"model": {"display_name": "Fable"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	var fable *Window
	for i := range q.Windows {
		if q.Windows[i].ID == WindowFable {
			fable = &q.Windows[i]
		}
	}
	if fable == nil || fable.UsedPct != 41 || fable.Label != "Fable (7-day)" {
		t.Fatalf("fable = %+v, want inactive weekly_scoped Fable at 41%%", fable)
	}
}

func TestParseClaudeUsageModelScopedFable(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 8},
		"seven_day": {"utilization": 2},
		"rate_limits": {
			"model_scoped": [
				{"display_name": "Fable", "utilization": 37, "resets_at": "2026-08-31T23:59:59Z"}
			]
		}
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	var fable *Window
	for i := range q.Windows {
		if q.Windows[i].ID == WindowFable {
			fable = &q.Windows[i]
		}
	}
	if fable == nil || fable.UsedPct != 37 || fable.Label != "Fable (7-day)" {
		t.Fatalf("parseClaudeUsage(model_scoped Fable) = %+v, want 37%% Fable (7-day)", fable)
	}
}

func TestParseClaudeUsageSkipsInactiveSessionLimit(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"limits": [
			{"kind": "session", "percent": 0, "is_active": false},
			{"kind": "session", "percent": 18, "is_active": true},
			{"kind": "weekly_all", "percent": 0, "is_active": false},
			{"kind": "weekly_all", "percent": 22, "is_active": true}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	five := windowByID(t, q.Windows, WindowFiveHour)
	week := windowByID(t, q.Windows, WindowSevenDay)
	if five.UsedPct != 18 {
		t.Errorf("parseClaudeUsage(inactive then active session).UsedPct = %v, want 18", five.UsedPct)
	}
	if week.UsedPct != 22 {
		t.Errorf("parseClaudeUsage(inactive then active weekly_all).UsedPct = %v, want 22", week.UsedPct)
	}
}

func TestParseClaudeUsageKeepsInactiveSharedLimitsWhenScopedBinds(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"limits": [
			{"kind": "session", "percent": 30, "is_active": false},
			{"kind": "weekly_all", "percent": 40, "is_active": false},
			{"kind": "weekly_scoped", "percent": 90, "is_active": true,
			 "scope": {"model": {"display_name": "Fable"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	five := windowByID(t, q.Windows, WindowFiveHour)
	week := windowByID(t, q.Windows, WindowSevenDay)
	fable := windowByID(t, q.Windows, WindowFable)
	if five.UsedPct != 30 {
		t.Errorf("parseClaudeUsage(scoped binding, inactive session).UsedPct = %v, want 30", five.UsedPct)
	}
	if week.UsedPct != 40 {
		t.Errorf("parseClaudeUsage(scoped binding, inactive weekly_all).UsedPct = %v, want 40", week.UsedPct)
	}
	if fable.UsedPct != 90 {
		t.Errorf("parseClaudeUsage(scoped binding Fable).UsedPct = %v, want 90", fable.UsedPct)
	}
}

func TestParseClaudeUsageKeepsVersionedScopedWindows(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 1},
		"seven_day": {"utilization": 2},
		"limits": [
			{"kind": "weekly_scoped", "percent": 80, "is_active": false,
			 "scope": {"model": {"display_name": "Opus 4.5"}}},
			{"kind": "weekly_scoped", "percent": 10, "is_active": true,
			 "scope": {"model": {"display_name": "Opus 5"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	older := windowByID(t, q.Windows, "seven_day_opus_4.5")
	newer := windowByID(t, q.Windows, "seven_day_opus_5")
	if older.UsedPct != 80 || older.Label != "Opus 4.5 (7-day)" {
		t.Errorf("parseClaudeUsage(Opus 4.5 scoped) = %+v, want 80%% Opus 4.5 (7-day)", older)
	}
	if newer.UsedPct != 10 || newer.Label != "Opus 5 (7-day)" {
		t.Errorf("parseClaudeUsage(Opus 5 scoped) = %+v, want 10%% Opus 5 (7-day)", newer)
	}
}

func TestParseClaudeUsageKeepsDotlessVersionedScopedWindows(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 1},
		"seven_day": {"utilization": 2},
		"limits": [
			{"kind": "weekly_scoped", "percent": 80, "is_active": false,
			 "scope": {"model": {"display_name": "Opus 4"}}},
			{"kind": "weekly_scoped", "percent": 10, "is_active": true,
			 "scope": {"model": {"display_name": "Opus 5"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	older := windowByID(t, q.Windows, "seven_day_opus_4")
	newer := windowByID(t, q.Windows, "seven_day_opus_5")
	if older.UsedPct != 80 || older.Label != "Opus 4 (7-day)" {
		t.Errorf("parseClaudeUsage(Opus 4 scoped) = %+v, want 80%% Opus 4 (7-day)", older)
	}
	if newer.UsedPct != 10 || newer.Label != "Opus 5 (7-day)" {
		t.Errorf("parseClaudeUsage(Opus 5 scoped) = %+v, want 10%% Opus 5 (7-day)", newer)
	}
}

func TestParseClaudeUsageMergesClaudePrefixedScopedName(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 1},
		"seven_day": {"utilization": 2},
		"limits": [
			{"kind": "weekly_scoped", "percent": 11,
			 "scope": {"model": {"display_name": "Opus 4.5"}}},
			{"kind": "weekly_scoped", "percent": 22,
			 "scope": {"model": {"display_name": "Claude Opus 4.5"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	n := 0
	for _, w := range q.Windows {
		if strings.Contains(w.ID, "opus") {
			n++
		}
	}
	if n != 1 {
		t.Errorf("parseClaudeUsage(Opus 4.5 + Claude Opus 4.5) opus windows = %d, want 1; windows = %+v", n, q.Windows)
	}
	w := windowByID(t, q.Windows, "seven_day_opus_4.5")
	if w.UsedPct != 11 {
		t.Errorf("parseClaudeUsage(prefixed name after short name).UsedPct = %v, want 11", w.UsedPct)
	}
}

func TestParseClaudeUsagePrefersActiveScopedLimit(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 1},
		"seven_day": {"utilization": 2},
		"limits": [
			{"kind": "weekly_scoped", "percent": 9, "is_active": false,
			 "scope": {"model": {"display_name": "Fable"}}},
			{"kind": "weekly_scoped", "percent": 44, "is_active": true,
			 "scope": {"model": {"display_name": "Fable"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	fable := windowByID(t, q.Windows, WindowFable)
	if fable.UsedPct != 44 {
		t.Errorf("parseClaudeUsage(inactive then active Fable).UsedPct = %v, want 44", fable.UsedPct)
	}
}

func TestParseClaudeUsageSkipsNonWeeklyModelScoped(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 8},
		"seven_day": {"utilization": 2},
		"model_scoped": [
			{"kind": "session", "display_name": "Fable", "utilization": 91},
			{"kind": "weekly_scoped", "display_name": "Fable", "utilization": 37}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	fable := windowByID(t, q.Windows, WindowFable)
	if fable.UsedPct != 37 {
		t.Errorf("parseClaudeUsage(session then weekly model_scoped).UsedPct = %v, want 37", fable.UsedPct)
	}
	if fable.Label != "Fable (7-day)" {
		t.Errorf("parseClaudeUsage(...).Label = %q, want %q", fable.Label, "Fable (7-day)")
	}
}

func TestParseClaudeUsageAliasesFableModelSlug(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"five_hour": {"utilization": 8},
		"seven_day": {"utilization": 2},
		"seven_day_fable": {"utilization": 12},
		"model_scoped": [
			{"name": "claude-fable-5", "utilization": 40,
			 "scope": {"model": {"display_name": "Fable"}}}
		]
	}`)
	q, err := parseClaudeUsage(raw)
	if err != nil {
		t.Fatalf("parseClaudeUsage: %v", err)
	}
	n := 0
	for _, w := range q.Windows {
		if w.ID == WindowFable || strings.Contains(w.ID, "fable") {
			n++
		}
	}
	if n != 1 {
		t.Errorf("parseClaudeUsage(top-level + slug model_scoped) fable windows = %d, want 1; windows = %+v", n, q.Windows)
	}
	fable := windowByID(t, q.Windows, WindowFable)
	if fable.UsedPct != 12 {
		t.Errorf("parseClaudeUsage(top-level Fable before model_scoped slug).UsedPct = %v, want 12", fable.UsedPct)
	}
	if fable.Label != "Fable (7-day)" {
		t.Errorf("parseClaudeUsage(...).Label = %q, want %q", fable.Label, "Fable (7-day)")
	}
}

func TestParseClaudeUsageLabelsFableSlug(t *testing.T) {
	t.Parallel()
	cases := []struct{ slug, id, label string }{
		{"claude-fable-5", "seven_day_fable_5", "Fable 5 (7-day)"},
		{"claude-fable-5-1", "seven_day_fable_5_1", "Fable 5.1 (7-day)"},
	}
	for _, tc := range cases {
		t.Run(tc.slug, func(t *testing.T) {
			raw := []byte(`{
				"five_hour": {"utilization": 8},
				"seven_day": {"utilization": 2},
				"model_scoped": [
					{"name": "` + tc.slug + `", "utilization": 40}
				]
			}`)
			q, err := parseClaudeUsage(raw)
			if err != nil {
				t.Fatalf("parseClaudeUsage: %v", err)
			}
			fable := windowByID(t, q.Windows, tc.id)
			if fable.UsedPct != 40 {
				t.Errorf("UsedPct = %v, want 40", fable.UsedPct)
			}
			if fable.Label != tc.label {
				t.Errorf("Label = %q, want %q", fable.Label, tc.label)
			}
		})
	}
}

func windowByID(t *testing.T, ws []Window, id string) *Window {
	t.Helper()
	for i := range ws {
		if ws[i].ID == id {
			return &ws[i]
		}
	}
	t.Fatalf("windowByID(%q) missing; windows = %+v", id, ws)
	return nil
}

func TestParseClaudeUsageNoWindows(t *testing.T) {
	t.Parallel()
	if _, err := parseClaudeUsage([]byte(`{"email":"a@b.c"}`)); err == nil {
		t.Fatal("parseClaudeUsage empty windows: want error")
	}
}

func TestParseClaudeAccountFile(t *testing.T) {
	t.Parallel()
	email := parseClaudeAccountFile([]byte(`{"oauthAccount":{"emailAddress":"dev@example.com","organizationName":"Acme"}}`))
	if email != "dev@example.com" {
		t.Errorf("email = %q", email)
	}
}

func TestParseCodexAuth(t *testing.T) {
	t.Parallel()
	a, err := parseCodexAuth([]byte(`{"tokens":{"access_token":"tok","account_id":"acc-1"}}`))
	if err != nil {
		t.Fatalf("parseCodexAuth: %v", err)
	}
	if a.AccessToken != "tok" || a.AccountID != "acc-1" || !a.HasChatGPT {
		t.Errorf("auth = %+v", a)
	}
	idTok := testJWT(t, map[string]any{"email": "codex@example.com"})
	withMail, err := parseCodexAuth([]byte(`{"tokens":{"access_token":"tok","id_token":"` + idTok + `"}}`))
	if err != nil {
		t.Fatalf("parseCodexAuth jwt: %v", err)
	}
	if withMail.Email != "codex@example.com" {
		t.Errorf("Email = %q", withMail.Email)
	}
	nestedTok := testJWT(t, map[string]any{
		"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "acc-jwt"},
	})
	fromJWT, err := parseCodexAuth([]byte(`{"tokens":{"access_token":"tok","id_token":"` + nestedTok + `"}}`))
	if err != nil {
		t.Fatalf("parseCodexAuth nested jwt: %v", err)
	}
	if fromJWT.AccountID != "acc-jwt" {
		t.Errorf("parseCodexAuth(nested chatgpt_account_id).AccountID = %q, want acc-jwt", fromJWT.AccountID)
	}
	keyOnly, err := parseCodexAuth([]byte(`{"OPENAI_API_KEY":"sk-x"}`))
	if err != nil {
		t.Fatalf("api key auth: %v", err)
	}
	if !keyOnly.HasAPIKey || keyOnly.HasChatGPT {
		t.Errorf("key-only = %+v", keyOnly)
	}
}

func TestParseCodexUsage(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"plan_type": "pro",
		"rate_limit": {
			"primary_window": {"used_percent": 33, "reset_at": 1778670307},
			"secondary_window": {"used_percent": 12, "reset_at": 1779275107}
		},
		"credits": {"has_credits": true, "unlimited": false, "balance": "45.25"}
	}`)
	q, err := parseCodexUsage(raw)
	if err != nil {
		t.Fatalf("parseCodexUsage: %v", err)
	}
	if q.Plan != "Pro" {
		t.Errorf("Plan = %q, want Pro", q.Plan)
	}
	if len(q.Windows) != 3 {
		t.Fatalf("windows = %d, want 3", len(q.Windows))
	}
	if q.Windows[0].ID != WindowFiveHour || q.Windows[0].UsedPct != 33 {
		t.Errorf("primary = %+v", q.Windows[0])
	}
	if q.Windows[2].Detail != "45.25" {
		t.Errorf("credits detail = %q", q.Windows[2].Detail)
	}
}

func TestParseCodexUsageWindowDuration(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		raw  string
		want []Window
	}{
		{
			name: "weekly-only primary (premium)",
			raw: `{
				"plan_type": "business",
				"rate_limit": {
					"primary_window": {"used_percent": 1, "limit_window_seconds": 604800, "reset_at": 1779275107}
				}
			}`,
			want: []Window{{
				ID: WindowSevenDay, Label: "7-day", UsedPct: 1, ResetsAt: 1779275107 * 1000,
			}},
		},
		{
			name: "plus windows with durations",
			raw: `{
				"rate_limit": {
					"primary_window": {"used_percent": 33, "limit_window_seconds": 18000, "reset_at": 1778670307},
					"secondary_window": {"used_percent": 12, "limit_window_seconds": 604800, "reset_at": 1779275107}
				}
			}`,
			want: []Window{
				{ID: WindowFiveHour, Label: "5-hour", UsedPct: 33, ResetsAt: 1778670307 * 1000},
				{ID: WindowSevenDay, Label: "7-day", UsedPct: 12, ResetsAt: 1779275107 * 1000},
			},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			q, err := parseCodexUsage([]byte(tc.raw))
			if err != nil {
				t.Fatalf("parseCodexUsage(%s): %v", tc.name, err)
			}
			if len(q.Windows) != len(tc.want) {
				t.Fatalf("parseCodexUsage(%s).Windows = %+v, want %+v", tc.name, q.Windows, tc.want)
			}
			for i, got := range q.Windows {
				if got.ID != tc.want[i].ID || got.Label != tc.want[i].Label || got.UsedPct != tc.want[i].UsedPct || got.ResetsAt != tc.want[i].ResetsAt {
					t.Errorf("parseCodexUsage(%s).Windows[%d] = %+v, want %+v", tc.name, i, got, tc.want[i])
				}
			}
		})
	}
}

func TestParseCodexUsageMonthlySpendControl(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"plan_type": "team",
		"rate_limit": {
			"primary_window": {"used_percent": 100, "reset_at": 1778670307},
			"secondary_window": {"used_percent": 16, "reset_at": 1779275107}
		},
		"credits": {"has_credits": true, "unlimited": false, "balance": null},
		"spend_control": {
			"reached": false,
			"individual_limit": {
				"source": "workspace_spend_controls",
				"limit": "500",
				"used": "500",
				"remaining": "0",
				"used_percent": 100,
				"reset_at": 1788220800
			}
		}
	}`)
	q, err := parseCodexUsage(raw)
	if err != nil {
		t.Fatalf("parseCodexUsage(spend_control) error = %v", err)
	}
	if hasWindow(q.Windows, WindowExtra) {
		t.Errorf("parseCodexUsage(null credit balance) windows = %+v, want no extra Credits bar", q.Windows)
	}
	var monthly *Window
	for i := range q.Windows {
		if q.Windows[i].ID == WindowBilling {
			monthly = &q.Windows[i]
		}
	}
	if monthly == nil {
		t.Fatalf("parseCodexUsage(spend_control) windows = %+v, want monthly billing window", q.Windows)
	}
	if monthly.Label != "Monthly" || monthly.UsedPct != 100 {
		t.Errorf("parseCodexUsage(spend_control) monthly = %+v, want Label Monthly UsedPct 100", monthly)
	}
	if monthly.Detail != "500 of 500 credits" {
		t.Errorf("parseCodexUsage(spend_control) monthly.Detail = %q, want %q", monthly.Detail, "500 of 500 credits")
	}
	if monthly.ResetsAt != 1788220800*1000 {
		t.Errorf("parseCodexUsage(spend_control) monthly.ResetsAt = %d, want %d", monthly.ResetsAt, int64(1788220800*1000))
	}
}

func TestParseCodexMonthlyUsage(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"current_month_usage": 297.4,
		"effective_monthly_limit": {
			"limit": 1000,
			"enforcement_mode": "HARD_CAP",
			"limit_mode": "amount_credits"
		}
	}`)
	w := parseCodexMonthlyUsage(raw)
	if w == nil {
		t.Fatal("parseCodexMonthlyUsage(...) = nil, want monthly window")
	}
	if w.ID != WindowBilling || w.Label != "Monthly" {
		t.Errorf("parseCodexMonthlyUsage(...).ID/Label = %q %q, want %q %q", w.ID, w.Label, WindowBilling, "Monthly")
	}
	if w.UsedPct != 29.74 {
		t.Errorf("parseCodexMonthlyUsage(...).UsedPct = %v, want 29.74", w.UsedPct)
	}
	if w.Detail != "297 of 1,000 credits" {
		t.Errorf("parseCodexMonthlyUsage(...).Detail = %q, want %q", w.Detail, "297 of 1,000 credits")
	}
	if got := parseCodexMonthlyUsage([]byte(`{"effective_monthly_limit":{"limit":0}}`)); got != nil {
		t.Errorf("parseCodexMonthlyUsage(zero limit) = %+v, want nil", got)
	}
	for _, mode := range []string{"none", "off", "disabled", "no_limit"} {
		raw := []byte(`{"current_month_usage":10,"effective_monthly_limit":{"limit":1000,"enforcement_mode":"` + mode + `"}}`)
		if got := parseCodexMonthlyUsage(raw); got != nil {
			t.Errorf("parseCodexMonthlyUsage(enforcement_mode %q) = %+v, want nil", mode, got)
		}
	}
}

func TestFormatCodexCreditQty(t *testing.T) {
	t.Parallel()
	cases := []struct {
		n    float64
		want string
	}{
		{n: 0, want: "0"},
		{n: 393, want: "393"},
		{n: 1000, want: "1,000"},
		{n: 297.4, want: "297"},
		{n: 150000, want: "150,000"},
	}
	for _, tc := range cases {
		if got := formatCodexCreditQty(tc.n); got != tc.want {
			t.Errorf("formatCodexCreditQty(%v) = %q, want %q", tc.n, got, tc.want)
		}
	}
	if got, want := formatCodexCredits(393, 1000), "393 of 1,000 credits"; got != want {
		t.Errorf("formatCodexCredits(393, 1000) = %q, want %q", got, want)
	}
}

func TestParseCursorPeriod(t *testing.T) {
	t.Parallel()
	raw := []byte(`{
		"billingCycleStart": "2026-08-18T00:00:00.000Z",
		"billingCycleEnd": "2026-09-18T00:00:00.000Z",
		"planUsage": {
			"totalSpend": 2550,
			"limit": 2000,
			"autoPercentUsed": 27.1,
			"apiPercentUsed": 0,
			"totalPercentUsed": 21.9
		}
	}`)
	period, err := parseCursorPeriod(raw)
	if err != nil {
		t.Fatalf("parseCursorPeriod: %v", err)
	}
	if period.Start != "2026-08-18T00:00:00.000Z" || !strings.HasPrefix(period.End, "2026-09-18") {
		t.Errorf("cycle = %q / %q", period.Start, period.End)
	}
	if period.AutoPct != 27.1 || period.APIPct != 0 || period.TotalPct != 21.9 {
		t.Errorf("pool pcts = auto %v api %v total %v", period.AutoPct, period.APIPct, period.TotalPct)
	}
	if len(period.Quota.Windows) != 1 {
		t.Fatalf("windows = %+v", period.Quota.Windows)
	}
	w := period.Quota.Windows[0]
	if w.UsedPct != 27.1 || w.Label != "Included usage" {
		t.Errorf("window = %+v, want Cursor Models pool 27.1 not blended 21.9", w)
	}
	if w.Detail != "" {
		t.Errorf("Detail = %q, want empty (percent, not dollars)", w.Detail)
	}
}

func TestParseCursorModels(t *testing.T) {
	t.Parallel()
	raw := []byte(`{"aggregations":[
		{"modelIntent":"cursor-grok-4.6-high-fast","inputTokens":"80","outputTokens":"20","totalCents":800,"tier":2},
		{"modelIntent":"composer-2.5","inputTokens":10,"outputTokens":0,"totalCents":200,"tier":2},
		{"modelIntent":"claude-4.6-opus","inputTokens":5,"outputTokens":5,"totalCents":100,"tier":1}
	]}`)
	rows := parseCursorModels(raw, 27.1, 5, 21.9)
	if len(rows) != 3 {
		t.Fatalf("rows = %+v", rows)
	}
	if rows[0].Name != "cursor-grok-4.6-high-fast" || rows[0].Tokens != 100 || rows[0].Bucket != BucketCursor {
		t.Errorf("first = %+v", rows[0])
	}
	// 800/1000 of the 27.1% Cursor Models pool — not 800/API-limit.
	if rows[0].UsedPct < 21.67 || rows[0].UsedPct > 21.69 {
		t.Errorf("first UsedPct = %v, want ~21.68 (share of auto pool)", rows[0].UsedPct)
	}
	if rows[1].Name != "composer-2.5" || rows[1].UsedPct < 5.41 || rows[1].UsedPct > 5.43 {
		t.Errorf("composer = %+v", rows[1])
	}
	if rows[2].Name != "claude-4.6-opus" || rows[2].Bucket != BucketOther || rows[2].UsedPct != 5 {
		t.Errorf("other = %+v", rows[2])
	}
}

func TestParseCursorAuthFile(t *testing.T) {
	t.Parallel()
	tok, err := parseCursorAuthFile([]byte(`{"accessToken":"sess","refreshToken":"r","email":"a@b.c","apiKey":"crsr_x"}`))
	if err != nil {
		t.Fatalf("parseCursorAuthFile: %v", err)
	}
	if tok.Access != "sess" || tok.Email != "a@b.c" || tok.APIKey != "crsr_x" {
		t.Errorf("tok = %+v", tok)
	}
	if _, err := parseCursorAuthFile([]byte(`{}`)); err == nil {
		t.Fatal("empty auth: want error")
	}
}

func TestMergeStaleKeepsLastWindows(t *testing.T) {
	t.Parallel()
	prev := Snapshot{FetchedAt: 100, Providers: []ProviderQuota{
		{ID: ProviderClaude, Windows: []Window{{ID: WindowFiveHour, UsedPct: 22}}},
	}}
	next := Snapshot{FetchedAt: 200, Providers: []ProviderQuota{
		{ID: ProviderClaude, Error: "rate_limited"},
	}}
	got := mergeStale(prev, next)
	if !got.Stale {
		t.Fatal("want stale")
	}
	if len(got.Providers) != 1 || len(got.Providers[0].Windows) != 1 {
		t.Fatalf("merged = %+v", got.Providers)
	}
	if got.Providers[0].Windows[0].UsedPct != 22 {
		t.Errorf("kept pct = %v", got.Providers[0].Windows[0].UsedPct)
	}
	if got.FetchedAt != 200 {
		t.Errorf("FetchedAt = %d, want attempt time 200 (not frozen last-known)", got.FetchedAt)
	}
}

func TestMergeStaleKeepsReachabilityFailure(t *testing.T) {
	t.Parallel()
	prev := Snapshot{FetchedAt: 100, Providers: []ProviderQuota{
		{ID: ProviderCodex, Windows: []Window{{ID: WindowFiveHour, UsedPct: 9}}},
	}}
	next := Snapshot{FetchedAt: 200, Providers: []ProviderQuota{
		{ID: ProviderCodex, Error: "Could not reach ChatGPT", Hint: "context deadline exceeded"},
	}}
	got := mergeStale(prev, next)
	if !got.Stale {
		t.Fatal("mergeStale(reachability) Stale = false, want true")
	}
	if len(got.Providers) != 1 || len(got.Providers[0].Windows) != 1 || got.Providers[0].Windows[0].UsedPct != 9 {
		t.Fatalf("mergeStale(reachability) = %+v, want last Codex windows", got.Providers)
	}
}

func TestMergeStaleDoesNotKeepAuthError(t *testing.T) {
	t.Parallel()
	prev := Snapshot{Providers: []ProviderQuota{
		{ID: ProviderClaude, Windows: []Window{{ID: WindowFiveHour, UsedPct: 4}}},
	}}
	next := Snapshot{Providers: []ProviderQuota{
		{ID: ProviderClaude, Error: "Claude Code login expired"},
	}}
	got := mergeStale(prev, next)
	if got.Stale {
		t.Fatal("mergeStale(auth error) Stale = true, want false")
	}
	if len(got.Providers) != 1 || got.Providers[0].Error != "Claude Code login expired" {
		t.Fatalf("mergeStale(auth error) = %+v, want login-expired row", got.Providers)
	}
}

func TestMergeStaleKeepsOmittedQuota(t *testing.T) {
	t.Parallel()
	prev := Snapshot{Providers: []ProviderQuota{
		{ID: ProviderClaude, Windows: []Window{{ID: WindowFiveHour, UsedPct: 12}}},
		{ID: ProviderCursor, Error: "No Cursor usage session"},
	}}
	next := Snapshot{FetchedAt: 9, Providers: []ProviderQuota{
		{ID: ProviderCodex, Windows: []Window{{ID: WindowFiveHour, UsedPct: 1}}},
	}}
	got := mergeStale(prev, next)
	ids := make([]string, len(got.Providers))
	for i, q := range got.Providers {
		ids[i] = q.ID
	}
	if len(got.Providers) != 2 || ids[0] != ProviderCodex || ids[1] != ProviderClaude {
		t.Errorf("mergeStale omitted = %v, want [codex claude] (keep windows, drop stub)", ids)
	}
}

func TestJWTNeedsRefresh(t *testing.T) {
	t.Parallel()
	jwt := func(exp int64) string {
		b, err := json.Marshal(map[string]any{"exp": exp})
		if err != nil {
			t.Fatalf("json.Marshal(exp=%d): %v", exp, err)
		}
		return "x." + base64.RawURLEncoding.EncodeToString(b) + ".y"
	}
	now := time.Now()
	cases := []struct {
		name string
		tok  string
		want bool
	}{
		{"opaque", "not-a-jwt", false},
		{"far future", jwt(4102444800), false},
		{"expired", jwt(1), true},
		{"6d left", jwt(now.Add(6 * 24 * time.Hour).Unix()), true},
		{"8d left", jwt(now.Add(8 * 24 * time.Hour).Unix()), false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			if got := jwtNeedsRefresh(tc.tok); got != tc.want {
				t.Errorf("jwtNeedsRefresh(%s) = %v, want %v", tc.name, got, tc.want)
			}
		})
	}
}

func TestAPIKeyIsNotCursorUsageToken(t *testing.T) {
	t.Parallel()
	f := &Fetcher{
		WantCursor: true,
		CursorToken: func() (cursorTok, error) {
			return cursorTok{}, nil
		},
	}
	q := fetchCursor(t.Context(), f)
	if q.Error == "" {
		t.Fatal("missing session should error")
	}
	if !strings.Contains(q.Hint, "Connect in Settings") {
		t.Errorf("Hint = %q, want Connect in Settings", q.Hint)
	}
}

func TestDefaultCursorTokenReadsConnectFile(t *testing.T) {
	t.Parallel()
	if _, err := defaultCursorToken(t.TempDir()); err == nil {
		t.Fatal("defaultCursorToken(empty dir) error = nil, want no session")
	}
	dir := t.TempDir()
	tok := cursorTok{Access: "recur-sess", Email: "recur@x.com", Source: SourceCursorConnect}
	if err := writeCursorAuthFile(cursorAuthPath(dir), tok); err != nil {
		t.Fatal(err)
	}
	got, err := defaultCursorToken(dir)
	if err != nil {
		t.Fatal(err)
	}
	if got.Access != "recur-sess" || got.Source != SourceCursorConnect {
		t.Errorf("defaultCursorToken = %+v, want Recur Connect token", got)
	}
}

func testJWT(t *testing.T, claims map[string]any) string {
	t.Helper()
	payload, err := json.Marshal(claims)
	if err != nil {
		t.Fatal(err)
	}
	return "eyJhbGciOiJub25lIn0." + base64.RawURLEncoding.EncodeToString(payload) + "."
}

func TestEmailFromCursorJSON(t *testing.T) {
	t.Parallel()
	if got := emailFromCursorJSON(map[string]any{"user": map[string]any{"email": "a@b.c"}}); got != "a@b.c" {
		t.Fatalf("nested = %q", got)
	}
	if got := emailFromCursorJSON(map[string]any{"cachedEmail": "c@d.e"}); got != "c@d.e" {
		t.Fatalf("cached = %q", got)
	}
	jwt := testJWT(t, map[string]any{"email": "jwt@cursor.dev"})
	if got := emailFromJWT(jwt); got != "jwt@cursor.dev" {
		t.Fatalf("jwt = %q", got)
	}
}

func TestUsedPctFromMissing(t *testing.T) {
	t.Parallel()
	if _, ok := usedPctFrom(nil); ok {
		t.Fatal("nil map should be missing")
	}
	if _, ok := usedPctFrom(map[string]any{"resets_at": 1}); ok {
		t.Fatal("no percent key should be missing")
	}
	if pct, ok := usedPctFrom(map[string]any{"utilization": 0}); !ok || pct != 0 {
		t.Fatalf("explicit 0 = %v %v", pct, ok)
	}
}

func TestParseCursorPeriodNoPercent(t *testing.T) {
	t.Parallel()
	_, err := parseCursorPeriod([]byte(`{"planUsage":{"totalSpend":1}}`))
	if err == nil {
		t.Fatal("want error when no percent fields")
	}
}

func TestParseCursorModelsDedupe(t *testing.T) {
	t.Parallel()
	raw := []byte(`{"aggregations":[
		{"modelIntent":"composer-2.5","inputTokens":10,"outputTokens":0,"totalCents":100,"tier":2},
		{"modelIntent":"composer-2.5","inputTokens":5,"outputTokens":5,"totalCents":50,"tier":2}
	]}`)
	rows := parseCursorModels(raw, 20, 0, 20)
	if len(rows) != 1 {
		t.Fatalf("rows = %+v, want 1 after dedupe", rows)
	}
	if rows[0].Tokens != 20 {
		t.Errorf("tokens = %d, want 20", rows[0].Tokens)
	}
}

func TestHasRecurCursorAuthFile(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	if HasRecurCursorAuthFile(dir) {
		t.Fatal("empty dir should have no Recur Connect file")
	}
	tok := cursorTok{Access: "sess", Source: SourceCursorConnect}
	if err := writeCursorAuthFile(cursorAuthPath(dir), tok); err != nil {
		t.Fatal(err)
	}
	if !HasRecurCursorAuthFile(dir) {
		t.Fatal("connect file should be detected")
	}
	if err := DeleteRecurCursorAuth(dir); err != nil {
		t.Fatal(err)
	}
	if HasRecurCursorAuthFile(dir) {
		t.Fatal("DeleteRecurCursorAuth should remove the file")
	}
}
