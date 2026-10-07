package usage

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const (
	defaultCursorLoginURL     = "https://cursor.com/loginDeepControl"
	defaultCursorPollURL      = "https://api2.cursor.sh/auth/poll"
	defaultCursorRefreshURL   = "https://api2.cursor.sh/auth/exchange_user_api_key"
	defaultCursorMintURL      = "https://api2.cursor.sh/aiserver.v1.DashboardService/CreateUserApiKey"
	defaultCursorListKeysURL  = "https://api2.cursor.sh/aiserver.v1.DashboardService/ListUserApiKeys"
	defaultCursorRevokeKeyURL = "https://api2.cursor.sh/aiserver.v1.DashboardService/RevokeUserApiKey"
	cursorConnectTimeout      = 3 * time.Minute
	cursorPollInterval        = 1500 * time.Millisecond
	cursorMintKeyTTL          = 90 * 24 * time.Hour
	cursorMintKeyName         = "Recur"
)

type pkceLogin struct {
	Verifier  string
	Challenge string
	UUID      string
	LoginURL  string
}

func generateCursorPKCE(loginBase string) (pkceLogin, error) {
	raw := make([]byte, 32)
	if _, err := rand.Read(raw); err != nil {
		return pkceLogin{}, err
	}
	verifier := base64.RawURLEncoding.EncodeToString(raw)
	sum := sha256.Sum256([]byte(verifier))
	challenge := base64.RawURLEncoding.EncodeToString(sum[:])
	id := make([]byte, 16)
	if _, err := rand.Read(id); err != nil {
		return pkceLogin{}, err
	}
	// UUID-shaped id without importing google/uuid (usage stays stdlib here).
	id[6] = (id[6] & 0x0f) | 0x40
	id[8] = (id[8] & 0x3f) | 0x80
	uuid := fmt.Sprintf("%x-%x-%x-%x-%x", id[0:4], id[4:6], id[6:8], id[8:10], id[10:16])
	// redirectTarget is Cursor's portal label, not a protocol switch.
	// sdk: "Sign in to an app using the Cursor SDK" / "return to your app".
	// cli made the page say Cursor CLI.
	q := url.Values{
		"challenge":      {challenge},
		"uuid":           {uuid},
		"mode":           {"login"},
		"redirectTarget": {"sdk"},
	}
	if loginBase == "" {
		loginBase = defaultCursorLoginURL
	}
	return pkceLogin{
		Verifier:  verifier,
		Challenge: challenge,
		UUID:      uuid,
		LoginURL:  loginBase + "?" + q.Encode(),
	}, nil
}

func pollCursorLogin(ctx context.Context, f *Fetcher, login pkceLogin) (cursorTok, error) {
	pollURL := f.cursorPollURL() + "?uuid=" + url.QueryEscape(login.UUID) + "&verifier=" + url.QueryEscape(login.Verifier)
	ticker := time.NewTicker(cursorPollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return cursorTok{}, ctx.Err()
		case <-ticker.C:
			status, raw, err := doJSON(ctx, f.client(), http.MethodGet, pollURL, nil, map[string]string{
				"Accept": "application/json",
			})
			if err != nil {
				continue
			}
			if status == http.StatusNoContent || status == http.StatusAccepted {
				continue
			}
			if status == http.StatusNotFound || status == http.StatusPaymentRequired {
				continue
			}
			if status >= 300 {
				continue
			}
			tok, err := parseCursorPoll(raw)
			if err != nil || tok.Access == "" {
				continue
			}
			tok.Source = SourceCursorConnect
			return tok, nil
		}
	}
}

func parseCursorPoll(raw []byte) (cursorTok, error) {
	var file struct {
		AccessToken  string `json:"accessToken"`
		RefreshToken string `json:"refreshToken"`
		Access       string `json:"access_token"`
		Refresh      string `json:"refresh_token"`
		Email        string `json:"email"`
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		return cursorTok{}, err
	}
	tok := cursorTok{
		Access:  firstNonEmpty(file.AccessToken, file.Access),
		Refresh: firstNonEmpty(file.RefreshToken, file.Refresh),
		Email:   strings.TrimSpace(file.Email),
		Source:  SourceCursorConnect,
	}
	if tok.Access == "" {
		return cursorTok{}, fmt.Errorf("poll returned no access token")
	}
	if tok.Email == "" {
		tok.Email = emailFromJWT(tok.Access)
	}
	return tok, nil
}

func refreshCursorToken(ctx context.Context, f *Fetcher, refresh string) (cursorTok, error) {
	if refresh == "" {
		return cursorTok{}, fmt.Errorf("no refresh token")
	}
	status, raw, err := doJSON(ctx, f.client(), http.MethodPost, f.cursorRefreshURL(), strings.NewReader("{}"), map[string]string{
		"Authorization": "Bearer " + refresh,
		"Content-Type":  "application/json",
		"Accept":        "application/json",
	})
	if err != nil {
		return cursorTok{}, err
	}
	if status >= 300 {
		return cursorTok{}, httpErr(status, raw)
	}
	tok, err := parseCursorPoll(raw)
	if err != nil {
		return cursorTok{}, err
	}
	if tok.Refresh == "" {
		tok.Refresh = refresh
	}
	tok.Source = SourceCursorConnect
	return tok, nil
}

func cursorAPIKeyNeedsRefresh(tok cursorTok, now time.Time) bool {
	if tok.APIKey == "" {
		return true
	}
	if tok.APIKeyExpiresAtMs <= 0 {
		return false
	}
	return now.Add(cursorRefreshLeeway).UnixMilli() >= tok.APIKeyExpiresAtMs
}

func (f *Fetcher) ensureCursorAPIKey(ctx context.Context, tok cursorTok) cursorTok {
	if tok.Access == "" || !cursorAPIKeyNeedsRefresh(tok, f.now()) {
		return tok
	}
	prev, prevID := tok.APIKey, tok.APIKeyID
	key, exp, err := mintCursorUserAPIKey(ctx, f, tok.Access)
	if err != nil || key == "" {
		return tok
	}
	tok.APIKey = key
	tok.APIKeyExpiresAtMs = exp
	tok.APIKeyID = 0
	if dir := f.dataDir(); dir != "" {
		_ = writeCursorAuthFile(cursorAuthPath(dir), tok)
	}
	if f.OnMintedAPIKey != nil {
		f.OnMintedAPIKey(prev, key)
	}
	if prev != "" {
		_ = revokeCursorUserAPIKey(ctx, f, tok.Access, prev, prevID)
	}
	if id := lookupCursorAPIKeyID(ctx, f, tok.Access, key); id != 0 {
		tok.APIKeyID = id
		if dir := f.dataDir(); dir != "" {
			_ = writeCursorAuthFile(cursorAuthPath(dir), tok)
		}
	}
	return tok
}

func mintCursorUserAPIKey(ctx context.Context, f *Fetcher, access string) (string, int64, error) {
	if access == "" {
		return "", 0, fmt.Errorf("no session token")
	}
	exp := f.now().Add(cursorMintKeyTTL).UnixMilli()
	body, err := json.Marshal(map[string]any{
		"name":        cursorMintKeyName,
		"expiresAtMs": exp,
	})
	if err != nil {
		return "", 0, err
	}
	status, raw, err := doJSON(ctx, f.client(), http.MethodPost, f.cursorMintURL(), bytes.NewReader(body), cursorAPIHeaders(access))
	if err != nil {
		return "", 0, err
	}
	if status >= 300 {
		return "", 0, httpErr(status, raw)
	}
	m, err := decodeMap(raw)
	if err != nil {
		return "", 0, err
	}
	key := firstString(m, "apiKey", "api_key")
	if key == "" {
		return "", 0, fmt.Errorf("mint returned no api key")
	}
	return key, exp, nil
}

type cursorAPIKeyRow struct {
	ID      int64
	Name    string
	Masked  string
	Created float64
}

func listCursorUserAPIKeys(ctx context.Context, f *Fetcher, access string) []cursorAPIKeyRow {
	status, raw, err := doJSON(ctx, f.client(), http.MethodPost, f.cursorListKeysURL(), bytes.NewReader([]byte("{}")), cursorAPIHeaders(access))
	if err != nil || status >= 300 {
		return nil
	}
	m, err := decodeMap(raw)
	if err != nil {
		return nil
	}
	rows := sliceOf(m["apiKeys"])
	if len(rows) == 0 {
		rows = sliceOf(m["api_keys"])
	}
	out := make([]cursorAPIKeyRow, 0, len(rows))
	for _, row := range rows {
		rm := mapOf(row)
		if rm == nil {
			continue
		}
		id := asInt64(firstAny(rm, "id"))
		if id == 0 {
			continue
		}
		out = append(out, cursorAPIKeyRow{
			ID:      id,
			Name:    firstString(rm, "name"),
			Masked:  firstString(rm, "maskedKey", "masked_key"),
			Created: asFloat(firstAny(rm, "createdAt", "created_at")),
		})
	}
	return out
}

func matchesMaskedCursorKey(full, masked string) bool {
	full, masked = strings.TrimSpace(full), strings.TrimSpace(masked)
	if full == "" || masked == "" {
		return false
	}
	i := strings.LastIndex(masked, "...")
	if i < 0 {
		return full == masked
	}
	suffix := masked[i+3:]
	return suffix != "" && strings.HasSuffix(full, suffix)
}

func lookupCursorAPIKeyID(ctx context.Context, f *Fetcher, access, fullKey string) int64 {
	var best int64
	var bestCreated float64
	for _, k := range listCursorUserAPIKeys(ctx, f, access) {
		if k.Name != "" && k.Name != cursorMintKeyName {
			continue
		}
		if !matchesMaskedCursorKey(fullKey, k.Masked) {
			continue
		}
		if best == 0 || k.Created >= bestCreated {
			best, bestCreated = k.ID, k.Created
		}
	}
	return best
}

func revokeCursorUserAPIKey(ctx context.Context, f *Fetcher, access, fullKey string, id int64) error {
	if access == "" {
		return fmt.Errorf("no session token")
	}
	if id == 0 {
		id = lookupCursorAPIKeyID(ctx, f, access, fullKey)
	}
	if id == 0 {
		return fmt.Errorf("cursor api key id not found")
	}
	body, err := json.Marshal(map[string]any{"id": id})
	if err != nil {
		return err
	}
	status, raw, err := doJSON(ctx, f.client(), http.MethodPost, f.cursorRevokeKeyURL(), bytes.NewReader(body), cursorAPIHeaders(access))
	if err != nil {
		return err
	}
	if status >= 300 {
		return httpErr(status, raw)
	}
	return nil
}

func firstNonEmpty(ss ...string) string {
	for _, s := range ss {
		if strings.TrimSpace(s) != "" {
			return strings.TrimSpace(s)
		}
	}
	return ""
}
