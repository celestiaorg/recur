package usage

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"recur/internal/codex"
)

const defaultCodexUsageURL = "https://chatgpt.com/backend-api/wham/usage"

type codexAuth struct {
	AccessToken string
	IDToken     string
	AccountID   string
	Email       string
	HasAPIKey   bool
	HasChatGPT  bool
}

func defaultCodexAuth(dataDir string) (codexAuth, error) {
	var lastErr error
	if dataDir != "" {
		if a, err := readCodexAuthFile(filepath.Join(dataDir, "codex-home", "auth.json")); err == nil && a.AccessToken != "" {
			return a, nil
		} else if err != nil {
			lastErr = err
		}
	}
	if home, err := os.UserHomeDir(); err == nil {
		if a, err := readCodexAuthFile(filepath.Join(home, ".codex", "auth.json")); err == nil && a.AccessToken != "" {
			return a, nil
		} else if err != nil {
			lastErr = err
		}
	}
	if lastErr != nil {
		return codexAuth{}, lastErr
	}
	return codexAuth{}, fmt.Errorf("no Codex ChatGPT auth.json")
}

func readCodexAuthFile(path string) (codexAuth, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return codexAuth{}, err
	}
	return parseCodexAuth(raw)
}

func parseCodexAuth(raw []byte) (codexAuth, error) {
	var file struct {
		Tokens *struct {
			AccessToken  string `json:"access_token"`
			RefreshToken string `json:"refresh_token"`
			IDToken      string `json:"id_token"`
			AccountID    string `json:"account_id"`
		} `json:"tokens"`
		IDToken      string `json:"id_token"`
		OPENAIAPIKey string `json:"OPENAI_API_KEY"`
		Email        string `json:"email"`
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		return codexAuth{}, fmt.Errorf("codex auth.json: %w", err)
	}
	out := codexAuth{Email: strings.TrimSpace(file.Email)}
	if file.Tokens != nil {
		out.AccessToken = strings.TrimSpace(file.Tokens.AccessToken)
		out.IDToken = strings.TrimSpace(file.Tokens.IDToken)
		out.AccountID = strings.TrimSpace(file.Tokens.AccountID)
		out.HasChatGPT = out.AccessToken != ""
		if out.Email == "" {
			out.Email = emailFromJWT(file.Tokens.IDToken)
		}
		if out.Email == "" {
			out.Email = emailFromJWT(file.Tokens.AccessToken)
		}
	}
	if out.IDToken == "" {
		out.IDToken = strings.TrimSpace(file.IDToken)
	}
	if out.Email == "" {
		out.Email = emailFromJWT(file.IDToken)
	}
	if out.AccountID == "" {
		out.AccountID = accountIDFromJWT(out.AccessToken)
	}
	if out.AccountID == "" {
		out.AccountID = accountIDFromJWT(out.IDToken)
	}
	if strings.TrimSpace(file.OPENAIAPIKey) != "" {
		out.HasAPIKey = true
	}
	if !out.HasChatGPT && !out.HasAPIKey {
		return codexAuth{}, fmt.Errorf("codex auth.json has no tokens")
	}
	return out, nil
}

func fetchCodex(ctx context.Context, f *Fetcher) ProviderQuota {
	out := ProviderQuota{ID: ProviderCodex, Name: "Codex"}
	auth, err := f.codexAuth()
	if err != nil || (!auth.HasChatGPT && !auth.HasAPIKey) {
		out.Error = "Not signed in to Codex"
		out.Hint = "Sign in with ChatGPT in Settings → Providers → Codex."
		return out
	}
	if auth.HasAPIKey && !auth.HasChatGPT {
		setSource(&out, SourceCodexAPIKey)
		out.Error = "API key billing — no plan quota"
		out.Hint = "Codex API keys are pay-as-you-go. Sign in with ChatGPT to see 5-hour and 7-day limits."
		return out
	}
	setSource(&out, SourceCodexChatGPT)
	headers := map[string]string{
		"Authorization": "Bearer " + auth.AccessToken,
		"Accept":        "application/json",
		"originator":    "codex_cli_rs",
		"User-Agent":    "codex_cli_rs",
	}
	if auth.AccountID != "" {
		headers["ChatGPT-Account-Id"] = auth.AccountID
		headers["chatgpt-account-id"] = auth.AccountID
	}
	status, raw, err := doJSON(ctx, f.client(), http.MethodGet, f.codexUsageURL(), nil, headers)
	if err != nil {
		out.Error = "Could not reach ChatGPT"
		out.Hint = err.Error()
		return out
	}
	if isRateLimited(status) {
		return ProviderQuota{ID: ProviderCodex, Name: "Codex", Error: "rate_limited"}
	}
	if isAuthErr(status) {
		out.Error = "Codex login expired"
		out.Hint = "Sign in with ChatGPT again in Settings → Providers → Codex."
		return out
	}
	if status >= 300 {
		out.Error = "Codex usage unavailable"
		out.Hint = httpErr(status, raw).Error()
		return out
	}
	q, err := parseCodexUsage(raw)
	if err != nil {
		out.Error = "Codex usage response was unreadable"
		return out
	}
	accountID := resolveCodexAccountID(auth, raw)
	if accountID != "" && headers["ChatGPT-Account-Id"] == "" {
		headers["ChatGPT-Account-Id"] = accountID
		headers["chatgpt-account-id"] = accountID
	}
	if !hasWindow(q.Windows, WindowBilling) && codexSpendControlPresent(raw) {
		if monthly := fetchCodexMonthly(ctx, f, headers, accountID); monthly != nil {
			q.Windows = append(q.Windows, *monthly)
		}
	}
	setSource(&q, SourceCodexChatGPT)
	if q.Email == "" {
		q.Email = auth.Email
	}
	return q
}

func fetchCodexMonthly(ctx context.Context, f *Fetcher, headers map[string]string, accountID string) *Window {
	monthlyURL := f.codexMonthlyURL(accountID)
	if monthlyURL == "" {
		return nil
	}
	status, raw, err := doJSON(ctx, f.client(), http.MethodGet, monthlyURL, nil, headers)
	if err != nil || status >= 300 {
		return nil
	}
	return parseCodexMonthlyUsage(raw)
}

func parseCodexUsage(raw []byte) (ProviderQuota, error) {
	m, err := decodeMap(raw)
	if err != nil {
		return ProviderQuota{}, fmt.Errorf("codex usage: %w", err)
	}
	q := ProviderQuota{ID: ProviderCodex, Name: "Codex"}
	setSource(&q, SourceCodexChatGPT)
	if plan := firstString(m, "plan_type", "planType"); plan != "" {
		q.Plan = codex.FormatPlan(plan)
	}
	q.Email = firstString(m, "email", "user_email", "userEmail")
	rl := mapOf(m["rate_limit"])
	if rl == nil {
		rl = m
	}
	if w := windowFromCodexRateLimit(mapOf(rl["primary_window"]), WindowFiveHour, "5-hour"); w != nil {
		q.Windows = append(q.Windows, *w)
	}
	if w := windowFromCodexRateLimit(mapOf(rl["secondary_window"]), WindowSevenDay, "7-day"); w != nil {
		q.Windows = append(q.Windows, *w)
	}
	if monthly := parseCodexIndividualLimit(m); monthly != nil {
		q.Windows = append(q.Windows, *monthly)
	}
	if credits := mapOf(m["credits"]); credits != nil {
		if asBool(credits["has_credits"]) && !asBool(credits["unlimited"]) {
			detail := asString(credits["balance"])
			if detail != "" {
				q.Windows = append(q.Windows, Window{
					ID:      WindowExtra,
					Label:   "Credits",
					UsedPct: 0,
					Detail:  detail,
				})
			}
		}
	}
	if len(q.Windows) == 0 {
		return ProviderQuota{}, fmt.Errorf("codex usage: no windows")
	}
	return q, nil
}

func codexSpendControlPresent(raw []byte) bool {
	m, err := decodeMap(raw)
	if err != nil {
		return false
	}
	return firstMap(m, "spend_control", "spendControl") != nil
}

func resolveCodexAccountID(auth codexAuth, usage []byte) string {
	if id := strings.TrimSpace(auth.AccountID); id != "" {
		return id
	}
	if m, err := decodeMap(usage); err == nil {
		if id := firstString(m, "account_id", "accountId"); id != "" {
			return id
		}
	}
	if id := accountIDFromJWT(auth.AccessToken); id != "" {
		return id
	}
	return accountIDFromJWT(auth.IDToken)
}

func accountIDFromJWT(tok string) string {
	m := jwtPayload(tok)
	if m == nil {
		return ""
	}
	if auth := mapOf(m["https://api.openai.com/auth"]); auth != nil {
		if id := firstString(auth, "chatgpt_account_id", "account_id", "accountId"); id != "" {
			return id
		}
	}
	return firstString(m, "chatgpt_account_id", "account_id", "accountId")
}

func parseCodexIndividualLimit(m map[string]any) *Window {
	limit := firstMap(m, "individual_limit", "individualLimit")
	if limit == nil {
		if sc := firstMap(m, "spend_control", "spendControl"); sc != nil {
			limit = firstMap(sc, "individual_limit", "individualLimit")
		}
	}
	if limit == nil {
		if rl := mapOf(m["rate_limit"]); rl != nil {
			limit = firstMap(rl, "individual_limit", "individualLimit")
		}
	}
	return windowFromCodexLimit(limit)
}

func parseCodexMonthlyUsage(raw []byte) *Window {
	m, err := decodeMap(raw)
	if err != nil {
		return nil
	}
	eff := firstMap(m, "effective_monthly_limit", "effectiveMonthlyLimit")
	if eff == nil {
		return nil
	}
	if !codexLimitEnforced(firstString(eff, "enforcement_mode", "enforcementMode")) {
		return nil
	}
	used := asFloat(firstAny(m, "current_month_usage", "currentMonthUsage"))
	cap := asFloat(firstAny(eff, "limit"))
	return windowFromCodexCredits(used, cap, 0)
}

func windowFromCodexRateLimit(block map[string]any, fallbackID, fallbackLabel string) *Window {
	if block == nil {
		return nil
	}
	pct, ok := usedPctFrom(block)
	if !ok {
		return nil
	}
	id, label := fallbackID, fallbackLabel
	switch int64(asFloat(firstAny(block, "limit_window_seconds", "limitWindowSeconds"))) {
	case 5 * 60 * 60:
		id, label = WindowFiveHour, "5-hour"
	case 7 * 24 * 60 * 60:
		id, label = WindowSevenDay, "7-day"
	}
	return &Window{
		ID:       id,
		Label:    label,
		UsedPct:  pct,
		ResetsAt: parseResetsAt(firstAny(block, "reset_at", "resets_at", "resetAt")),
	}
}

func windowFromCodexLimit(limit map[string]any) *Window {
	if limit == nil {
		return nil
	}
	if !codexLimitEnforced(firstString(limit, "enforcement_mode", "enforcementMode")) {
		return nil
	}
	cap := asFloat(firstAny(limit, "limit"))
	used := asFloat(firstAny(limit, "used"))
	resets := parseResetsAt(firstAny(limit, "reset_at", "resets_at", "resetAt", "resetsAt"))
	pct, ok := usedPctFrom(limit)
	if !ok && cap > 0 {
		pct = clampPct(used / cap * 100)
		ok = true
	}
	if !ok {
		return nil
	}
	w := Window{
		ID:       WindowBilling,
		Label:    "Monthly",
		UsedPct:  pct,
		ResetsAt: resets,
	}
	if cap > 0 {
		w.Detail = formatCodexCredits(used, cap)
	}
	return &w
}

func windowFromCodexCredits(used, cap float64, resets int64) *Window {
	if cap <= 0 {
		return nil
	}
	return &Window{
		ID:       WindowBilling,
		Label:    "Monthly",
		UsedPct:  clampPct(used / cap * 100),
		ResetsAt: resets,
		Detail:   formatCodexCredits(used, cap),
	}
}

func formatCodexCredits(used, cap float64) string {
	return fmt.Sprintf("%s of %s credits", formatCodexCreditQty(used), formatCodexCreditQty(cap))
}

func formatCodexCreditQty(n float64) string {
	n64 := int64(math.Round(n))
	neg := n64 < 0
	if neg {
		n64 = -n64
	}
	s := strconv.FormatInt(n64, 10)
	var b strings.Builder
	b.Grow(len(s) + len(s)/3 + 1)
	if neg {
		b.WriteByte('-')
	}
	for i := range s {
		if i > 0 && (len(s)-i)%3 == 0 {
			b.WriteByte(',')
		}
		b.WriteByte(s[i])
	}
	return b.String()
}

func codexMonthlyUsageURL(usageURL, accountID string) string {
	accountID = strings.TrimSpace(accountID)
	if usageURL == "" || accountID == "" {
		return ""
	}
	u, err := url.Parse(usageURL)
	if err != nil {
		return ""
	}
	path := strings.TrimSuffix(u.Path, "/")
	path = strings.TrimSuffix(path, "/wham/usage")
	u.Path = strings.TrimSuffix(path, "/") + "/accounts/" + accountID + "/spend-controls/current-user/monthly-usage"
	u.RawQuery = ""
	u.Fragment = ""
	return u.String()
}

func asBool(v any) bool {
	switch t := v.(type) {
	case bool:
		return t
	case string:
		return strings.EqualFold(t, "true")
	}
	return asFloat(v) != 0
}

func codexLimitEnforced(mode string) bool {
	switch strings.ToLower(strings.TrimSpace(mode)) {
	case "none", "off", "disabled", "no_limit", "nolimit", "unlimited":
		return false
	default:
		return true
	}
}
