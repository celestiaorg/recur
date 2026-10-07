package usage

import (
	"context"
	"fmt"
	"net/http"
	"strings"
)

const defaultClaudeUsageURL = "https://api.anthropic.com/api/oauth/usage"

func fetchClaude(ctx context.Context, f *Fetcher) ProviderQuota {
	out := ProviderQuota{ID: ProviderClaude, Name: "Claude"}
	tok, nudged, err := f.claudeAccess(ctx)
	if err != nil || tok == "" {
		out.Error = "Not signed in to Claude Code"
		out.Hint = "Run claude login, then reopen Recur."
		return out
	}
	q, status := fetchClaudeUsage(ctx, f, tok)
	// 401 with a still-unexpired token: one rotation, then retry if the
	// access token actually changed. Skip a second `claude auth status` when
	// the proactive expiry nudge already ran, and skip 403 (org/policy).
	if status != http.StatusUnauthorized || nudged || f.ClaudeToken != nil {
		return q
	}
	f.nudgeClaude(ctx)
	next := f.claudeCreds(ctx).Access
	if next == "" || next == tok {
		return q
	}
	q, _ = fetchClaudeUsage(ctx, f, next)
	return q
}

func fetchClaudeUsage(ctx context.Context, f *Fetcher, tok string) (ProviderQuota, int) {
	out := ProviderQuota{ID: ProviderClaude, Name: "Claude"}
	status, raw, err := doJSON(ctx, f.client(), http.MethodGet, f.claudeUsageURL(), nil, map[string]string{
		"Authorization":  "Bearer " + tok,
		"anthropic-beta": "oauth-2025-04-20",
		"User-Agent":     "claude-code/2.1.80",
		"Accept":         "application/json",
	})
	if err != nil {
		out.Error = "Could not reach Anthropic"
		out.Hint = err.Error()
		return out, status
	}
	if isRateLimited(status) {
		return ProviderQuota{ID: ProviderClaude, Name: "Claude", Error: "rate_limited"}, status
	}
	if isAuthErr(status) {
		out.Error = "Claude Code login expired"
		out.Hint = "Run claude login, then reopen Recur."
		return out, status
	}
	if status >= 300 {
		out.Error = "Claude usage unavailable"
		out.Hint = httpErr(status, raw).Error()
		return out, status
	}
	q, err := parseClaudeUsage(raw)
	if err != nil {
		out.Error = "Claude usage response was unreadable"
		return out, status
	}
	setSource(&q, SourceClaudeLogin)
	if q.Email == "" {
		q.Email = defaultClaudeAccount()
	}
	return q, status
}

func parseClaudeUsage(raw []byte) (ProviderQuota, error) {
	m, err := decodeMap(raw)
	if err != nil {
		return ProviderQuota{}, fmt.Errorf("claude usage: %w", err)
	}
	q := ProviderQuota{ID: ProviderClaude, Name: "Claude"}
	setSource(&q, SourceClaudeLogin)
	q.Email = firstString(m, "email", "user_email")
	if org := mapOf(m["organization"]); org != nil {
		if q.Email == "" {
			q.Email = asString(org["email"])
		}
	}
	if p := asString(m["subscription_type"]); p != "" {
		q.Plan = p
	}

	windows := []struct {
		keys  []string
		id    string
		label string
	}{
		{[]string{"five_hour", "fiveHour", "session"}, WindowFiveHour, "5-hour"},
		{[]string{"seven_day", "sevenDay", "weekly"}, WindowSevenDay, "7-day"},
		{[]string{"seven_day_opus", "sevenDayOpus"}, WindowOpus, "Opus (7-day)"},
		{[]string{"seven_day_sonnet", "sevenDaySonnet"}, WindowSonnet, "Sonnet (7-day)"},
		{[]string{"seven_day_fable", "sevenDayFable"}, WindowFable, "Fable (7-day)"},
	}
	for _, spec := range windows {
		block := firstMap(m, spec.keys...)
		if block == nil {
			continue
		}
		pct, ok := usedPctFrom(block)
		if !ok {
			continue
		}
		q.Windows = append(q.Windows, Window{
			ID:       spec.id,
			Label:    spec.label,
			UsedPct:  pct,
			ResetsAt: parseResetsAt(firstAny(block, "resets_at", "resetsAt", "reset_at")),
		})
	}
	inactive := map[string]bool{}
	appendClaudeLimits(&q, m, inactive)
	appendClaudeModelScoped(&q, m, inactive)
	if extra := parseClaudeExtra(firstMap(m, "extra", "extra_usage", "extraUsage")); extra != nil {
		q.Windows = append(q.Windows, *extra)
	}
	if len(q.Windows) == 0 {
		return ProviderQuota{}, fmt.Errorf("claude usage: no windows")
	}
	return q, nil
}

func parseClaudeExtra(block map[string]any) *Window {
	if block == nil {
		return nil
	}
	if v := firstAny(block, "is_enabled", "isEnabled"); v != nil && !asBool(v) {
		return nil
	}
	pct, _ := usedPctFrom(block)
	w := Window{
		ID:       WindowExtra,
		Label:    "Extra",
		UsedPct:  pct,
		ResetsAt: parseResetsAt(firstAny(block, "resets_at", "resetsAt", "reset_at")),
	}
	used := asFloat(firstAny(block, "used_credits", "usedCredits"))
	limit := asFloat(firstAny(block, "monthly_limit", "monthlyLimit"))
	if limit > 0 {
		w.UsedPct = clampPct(used / limit * 100)
		w.Detail = fmt.Sprintf("$%.2f of $%.2f", used/100, limit/100)
	}
	if w.UsedPct == 0 && w.Detail == "" && w.ResetsAt == 0 {
		return nil
	}
	return &w
}

func appendClaudeLimits(q *ProviderQuota, m map[string]any, inactive map[string]bool) {
	for _, row := range sliceOf(m["limits"]) {
		rm := mapOf(row)
		if rm == nil {
			continue
		}
		kind := firstString(rm, "kind")
		pct, ok := usedPctFrom(rm)
		if !ok {
			continue
		}
		active := claudeLimitActive(rm)
		resets := parseResetsAt(firstAny(rm, "resets_at", "resetsAt", "reset_at"))
		switch kind {
		case "session":
			// is_active marks the binding cap. A false session/weekly_all row is
			// still the real 5-hour/7-day bucket when a scoped cap is binding.
			appendClaudeWindow(q, WindowFiveHour, "5-hour", pct, resets, active, inactive)
		case "weekly_all", "weekly":
			appendClaudeWindow(q, WindowSevenDay, "7-day", pct, resets, active, inactive)
		case "weekly_scoped":
			appendScopedClaudeWindow(q, claudeScopeName(rm), pct, resets, active, inactive)
		}
	}
}

func appendClaudeModelScoped(q *ProviderQuota, m map[string]any, inactive map[string]bool) {
	rows := sliceOf(firstAny(m, "model_scoped", "modelScoped"))
	if len(rows) == 0 {
		if rl := mapOf(firstAny(m, "rate_limits", "rateLimits")); rl != nil {
			rows = sliceOf(firstAny(rl, "model_scoped", "modelScoped"))
		}
	}
	for _, row := range rows {
		rm := mapOf(row)
		if rm == nil || !claudeModelScopedWeekly(rm) {
			continue
		}
		pct, ok := usedPctFrom(rm)
		if !ok {
			continue
		}
		appendScopedClaudeWindow(q, claudeScopeName(rm), pct, parseResetsAt(firstAny(rm, "resets_at", "resetsAt", "reset_at")), claudeLimitActive(rm), inactive)
	}
}

func appendScopedClaudeWindow(q *ProviderQuota, name string, pct float64, resets int64, active bool, inactive map[string]bool) {
	if name == "" {
		return
	}
	id := scopedWindowID(name)
	appendClaudeWindow(q, id, scopedWindowLabel(name), pct, resets, active, inactive)
}

func appendClaudeWindow(q *ProviderQuota, id, label string, pct float64, resets int64, active bool, inactive map[string]bool) {
	if id == "" {
		return
	}
	w := Window{ID: id, Label: label, UsedPct: pct, ResetsAt: resets}
	if i := windowIndex(q.Windows, id); i >= 0 {
		if active && inactive != nil && inactive[id] {
			q.Windows[i] = w
			delete(inactive, id)
		}
		return
	}
	q.Windows = append(q.Windows, w)
	if !active && inactive != nil {
		inactive[id] = true
	}
}

func claudeLimitActive(rm map[string]any) bool {
	v := firstAny(rm, "is_active", "isActive")
	if v == nil {
		return true
	}
	return asBool(v)
}

func claudeModelScopedWeekly(rm map[string]any) bool {
	kind := strings.ToLower(strings.TrimSpace(firstString(rm, "kind", "period", "window")))
	if kind == "" {
		return true
	}
	switch kind {
	case "session", "five_hour", "5h", "5-hour":
		return false
	case "weekly_scoped", "weekly", "weekly_all", "seven_day", "7d", "7-day":
		return true
	default:
		return strings.Contains(kind, "week") || strings.Contains(kind, "seven")
	}
}

func claudeScopeName(rm map[string]any) string {
	if scope := mapOf(rm["scope"]); scope != nil {
		if model := mapOf(scope["model"]); model != nil {
			if name := firstString(model, "display_name", "displayName", "name"); name != "" {
				return name
			}
		}
	}
	return firstString(rm, "display_name", "displayName", "name")
}

func scopedWindowID(name string) string {
	s := canonicalScopedName(name)
	if s == "" {
		return ""
	}
	switch s {
	case "fable":
		return WindowFable
	case "opus":
		return WindowOpus
	case "sonnet":
		return WindowSonnet
	}
	return "seven_day_" + s
}

func canonicalScopedName(name string) string {
	s := strings.ToLower(strings.TrimSpace(name))
	s = strings.ReplaceAll(s, " ", "_")
	s = strings.ReplaceAll(s, "-", "_")
	return strings.TrimPrefix(s, "claude_")
}

func scopedWindowLabel(name string) string {
	s := canonicalScopedName(name)
	if s == "" {
		return strings.TrimSpace(name) + " (7-day)"
	}
	fam, rest := splitScopedFamily(s)
	if fam == "" {
		return humanizeScopedName(s) + " (7-day)"
	}
	label := fam
	if rest != "" {
		label += " " + prettyScopedVersion(rest)
	}
	return label + " (7-day)"
}

func splitScopedFamily(s string) (fam, rest string) {
	for _, name := range []string{"Fable", "Opus", "Sonnet"} {
		key := strings.ToLower(name)
		if s == key {
			return name, ""
		}
		if rest, ok := strings.CutPrefix(s, key+"_"); ok && rest != "" {
			return name, rest
		}
	}
	return "", ""
}

func prettyScopedVersion(rest string) string {
	if isModelIDSuffix(rest) {
		return strings.ReplaceAll(rest, "_", ".")
	}
	return strings.ReplaceAll(rest, "_", " ")
}

func humanizeScopedName(s string) string {
	parts := strings.Split(s, "_")
	for i, p := range parts {
		if p == "" {
			continue
		}
		parts[i] = strings.ToUpper(p[:1]) + p[1:]
	}
	return strings.Join(parts, " ")
}

func isModelIDSuffix(s string) bool {
	hasDigit := false
	for _, r := range s {
		if r == '_' {
			continue
		}
		if r < '0' || r > '9' {
			return false
		}
		hasDigit = true
	}
	return hasDigit
}

func windowIndex(ws []Window, id string) int {
	for i, w := range ws {
		if w.ID == id {
			return i
		}
	}
	return -1
}

func hasWindow(ws []Window, id string) bool {
	return windowIndex(ws, id) >= 0
}

func firstMap(m map[string]any, keys ...string) map[string]any {
	for _, k := range keys {
		if block := mapOf(m[k]); block != nil {
			return block
		}
	}
	return nil
}

func firstString(m map[string]any, keys ...string) string {
	for _, k := range keys {
		if s := asString(m[k]); s != "" {
			return s
		}
	}
	return ""
}

func firstAny(m map[string]any, keys ...string) any {
	for _, k := range keys {
		if v, ok := m[k]; ok && v != nil {
			return v
		}
	}
	return nil
}
