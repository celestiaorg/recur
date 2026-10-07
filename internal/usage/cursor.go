package usage

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
)

const (
	defaultCursorPeriodURL  = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage"
	defaultCursorAggURL     = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetAggregatedUsageEvents"
	defaultCursorProfileURL = "https://api2.cursor.sh/auth/full_stripe_profile"
	defaultCursorMeURL      = "https://api2.cursor.sh/auth/me"
	defaultCursorWebMeURL   = "https://cursor.com/api/auth/me"
	defaultCursorGetMeURL   = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetMe"
)

func fetchCursor(ctx context.Context, f *Fetcher) ProviderQuota {
	if f.cursorConnecting() {
		return CursorConnectingQuota()
	}
	out := ProviderQuota{ID: ProviderCursor, Name: "Cursor"}
	tok, err := f.cursorToken()
	if err != nil || tok.Access == "" {
		out.Error = "No Cursor usage session"
		out.Hint = cursorHintNoSession()
		return out
	}
	if refreshed, ok := f.maybeRefreshCursor(ctx, tok); ok {
		tok = refreshed
	}
	if tok.Source == SourceCursorConnect {
		tok = f.ensureCursorAPIKey(ctx, tok)
	}
	setSource(&out, tok.Source)
	out.Email = tok.Email

	headers := cursorAPIHeaders(tok.Access)
	status, raw, err := doJSON(ctx, f.client(), http.MethodPost, f.cursorPeriodURL(), bytes.NewReader([]byte("{}")), headers)
	if err != nil {
		out.Error = "Could not reach Cursor"
		out.Hint = err.Error()
		return out
	}
	if isRateLimited(status) {
		return ProviderQuota{ID: ProviderCursor, Name: "Cursor", Error: "rate_limited"}
	}
	if isAuthErr(status) {
		out.Error = "Cursor session expired"
		out.Hint = "Reconnect in Settings."
		return out
	}
	if status >= 300 {
		out.Error = "Cursor usage unavailable"
		out.Hint = httpErr(status, raw).Error()
		return out
	}
	period, err := parseCursorPeriod(raw)
	if err != nil {
		out.Error = "Cursor usage response was unreadable"
		return out
	}
	q := period.Quota
	setSource(&q, tok.Source)
	if q.Email == "" {
		q.Email = tok.Email
	}
	if q.Email == "" {
		q.Email = fetchCursorEmail(ctx, f, tok.Access)
	}

	aggHeaders := cursorAPIHeaders(tok.Access)
	body, _ := json.Marshal(map[string]any{
		"startDate": period.Start,
		"endDate":   period.End,
	})
	if ast, araw, aerr := doJSON(ctx, f.client(), http.MethodPost, f.cursorAggURL(), bytes.NewReader(body), aggHeaders); aerr == nil && ast < 300 {
		q.Models = parseCursorModels(araw, period.AutoPct, period.APIPct, period.TotalPct)
	}
	if q.Email != "" && tok.Source == SourceCursorConnect && tok.Email == "" {
		tok.Email = q.Email
		if dir := f.dataDir(); dir != "" {
			_ = writeCursorAuthFile(cursorAuthPath(dir), tok)
		}
	}
	return q
}

func cursorAPIHeaders(access string) map[string]string {
	return map[string]string{
		"Authorization":            "Bearer " + access,
		"Content-Type":             "application/json",
		"Accept":                   "application/json",
		"Connect-Protocol-Version": "1",
		"Origin":                   "https://cursor.com",
	}
}

type cursorPeriod struct {
	Quota    ProviderQuota
	Start    string
	End      string
	AutoPct  float64
	APIPct   float64
	TotalPct float64
}

func parseCursorPeriod(raw []byte) (cursorPeriod, error) {
	m, err := decodeMap(raw)
	if err != nil {
		return cursorPeriod{}, fmt.Errorf("cursor period: %w", err)
	}
	q := ProviderQuota{ID: ProviderCursor, Name: "Cursor"}
	q.Email = emailFromCursorJSON(m)
	start := firstString(m, "billingCycleStart", "billing_cycle_start")
	end := firstString(m, "billingCycleEnd", "billing_cycle_end")
	plan := mapOf(m["planUsage"])
	if plan == nil {
		plan = mapOf(m["plan_usage"])
	}
	if plan == nil {
		return cursorPeriod{}, fmt.Errorf("cursor period: no planUsage")
	}
	autoPct, autoOK := pctField(plan, "autoPercentUsed", "auto_percent_used")
	apiPct, apiOK := pctField(plan, "apiPercentUsed", "api_percent_used")
	totalPct, totalOK := pctField(plan, "totalPercentUsed", "total_percent_used")
	// The Included Usage table is two pools (Cursor Models vs Other Models).
	// totalPercentUsed is a blend and undershoots the dashboard headline;
	// planUsage.limit is the Other Models API-dollar cap, not the Cursor pool.
	var pct float64
	ok := autoOK || apiOK || totalOK
	if autoOK || apiOK {
		pct = autoPct
		if apiOK && apiPct > pct {
			pct = apiPct
		}
	} else if totalOK {
		pct = totalPct
	}
	if !ok {
		limit := asFloat(firstAny(plan, "limit"))
		used := asFloat(firstAny(plan, "totalSpend", "includedSpend", "total_spend", "included_spend"))
		if limit > 0 {
			pct = clampPct(used / limit * 100)
			ok = true
		}
	}
	if !ok {
		return cursorPeriod{}, fmt.Errorf("cursor period: no usage percent")
	}
	q.Windows = []Window{{
		ID:       WindowBilling,
		Label:    "Included usage",
		UsedPct:  pct,
		ResetsAt: parseResetsAt(end),
	}}
	return cursorPeriod{
		Quota:    q,
		Start:    start,
		End:      end,
		AutoPct:  autoPct,
		APIPct:   apiPct,
		TotalPct: totalPct,
	}, nil
}

func parseCursorModels(raw []byte, autoPct, apiPct, totalPct float64) []ModelRow {
	m, err := decodeMap(raw)
	if err != nil {
		return nil
	}
	rows := sliceOf(m["aggregations"])
	if len(rows) == 0 {
		rows = sliceOf(m["aggregation"])
	}
	type agg struct {
		row   ModelRow
		cents float64
	}
	var items []agg
	byName := map[string]int{}
	cursorCents, otherCents := 0.0, 0.0
	for _, row := range rows {
		rm := mapOf(row)
		if rm == nil {
			continue
		}
		name := firstString(rm, "modelIntent", "model", "name")
		if name == "" {
			continue
		}
		tokens := asInt64(firstAny(rm, "inputTokens", "input_tokens")) +
			asInt64(firstAny(rm, "outputTokens", "output_tokens")) +
			asInt64(firstAny(rm, "cacheReadTokens", "cache_read_tokens")) +
			asInt64(firstAny(rm, "cacheWriteTokens", "cache_write_tokens"))
		if tokens == 0 {
			tokens = asInt64(firstAny(rm, "totalTokens", "total_tokens"))
		}
		cents := asFloat(firstAny(rm, "totalCents", "total_cents"))
		if i, exists := byName[name]; exists {
			items[i].row.Tokens += tokens
			items[i].cents += cents
			if items[i].row.Bucket == BucketOther {
				otherCents += cents
			} else {
				cursorCents += cents
			}
			continue
		}
		item := agg{
			row: ModelRow{
				Name:   name,
				Tokens: tokens,
				Bucket: cursorModelBucket(name, rm["tier"]),
			},
			cents: cents,
		}
		if item.row.Bucket == BucketOther {
			otherCents += cents
		} else {
			cursorCents += cents
		}
		byName[name] = len(items)
		items = append(items, item)
	}
	sort.Slice(items, func(i, j int) bool {
		if items[i].cents != items[j].cents {
			return items[i].cents > items[j].cents
		}
		if items[i].row.Tokens != items[j].row.Tokens {
			return items[i].row.Tokens > items[j].row.Tokens
		}
		return items[i].row.Name < items[j].row.Name
	})
	cursorScale := autoPct
	if cursorScale == 0 {
		cursorScale = totalPct
	}
	out := make([]ModelRow, 0, len(items))
	for _, it := range items {
		sum, scale := cursorCents, cursorScale
		if it.row.Bucket == BucketOther {
			sum, scale = otherCents, apiPct
		}
		if sum > 0 && scale > 0 && it.cents > 0 {
			it.row.UsedPct = clampPct(it.cents / sum * scale)
		}
		out = append(out, it.row)
	}
	return out
}

func pctField(m map[string]any, keys ...string) (float64, bool) {
	v := firstAny(m, keys...)
	if v == nil {
		return 0, false
	}
	return clampPct(asFloat(v)), true
}

func cursorModelBucket(name string, tier any) string {
	if t := asInt64(tier); t == 1 {
		return BucketOther
	} else if t == 2 {
		return BucketCursor
	}
	n := strings.ToLower(strings.TrimSpace(name))
	switch {
	case strings.HasPrefix(n, "cursor-"),
		strings.HasPrefix(n, "composer-"),
		strings.HasPrefix(n, "grok-"),
		n == "auto":
		return BucketCursor
	default:
		return BucketOther
	}
}

func fetchCursorEmail(ctx context.Context, f *Fetcher, access string) string {
	if e := emailFromJWT(access); e != "" {
		return e
	}
	try := func(method, url string, body []byte, headers map[string]string) string {
		if url == "" {
			return ""
		}
		var r io.Reader
		if body != nil {
			r = bytes.NewReader(body)
		}
		status, raw, err := doJSON(ctx, f.client(), method, url, r, headers)
		if err != nil || status >= 300 {
			return ""
		}
		m, err := decodeMap(raw)
		if err != nil {
			return ""
		}
		return emailFromCursorJSON(m)
	}
	// Dashboard identity: cookie is `sub::access`, not a Bearer token.
	if sub := jwtClaim(access, "sub"); sub != "" {
		web := map[string]string{
			"Accept": "application/json",
			"Origin": "https://cursor.com",
			"Cookie": "WorkosCursorSessionToken=" + sub + "::" + access,
		}
		if e := try(http.MethodGet, f.cursorWebMeURL(), nil, web); e != "" {
			return e
		}
	}
	rest := map[string]string{
		"Authorization": "Bearer " + access,
		"Accept":        "application/json",
		"Origin":        "https://cursor.com",
	}
	if e := try(http.MethodGet, f.cursorProfileURL(), nil, rest); e != "" {
		return e
	}
	if e := try(http.MethodGet, f.cursorMeURL(), nil, rest); e != "" {
		return e
	}
	if e := try(http.MethodPost, f.cursorGetMeURL(), []byte("{}"), cursorAPIHeaders(access)); e != "" {
		return e
	}
	return ""
}
