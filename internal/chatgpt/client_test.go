package chatgpt

import (
	"context"
	"crypto"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type oauthFixture struct {
	t                                                   *testing.T
	c                                                   *Client
	key                                                 *rsa.PrivateKey
	mu                                                  sync.Mutex
	challenge, redirect, subject, workspace, tokenFault string
	exchanges, refreshes, polls, deviceStarts           atomic.Int32
	pendingStatuses                                     []int
	deviceFailure                                       int
}

func TestCodexOAuthEndpoints(t *testing.T) {
	c := New(t.TempDir())
	if c.authorizeURL != "https://auth.openai.com/oauth/authorize" || c.tokenURL != "https://auth.openai.com/oauth/token" {
		t.Fatal("browser OAuth is not using the Codex endpoints")
	}
	if c.verificationURL != "https://auth.openai.com/codex/device" || c.deviceCodeURL != "https://auth.openai.com/api/accounts/deviceauth/usercode" || c.deviceTokenURL != "https://auth.openai.com/api/accounts/deviceauth/token" {
		t.Fatal("headless OAuth is not using the Codex device flow")
	}
	if !strings.HasPrefix(c.modelsURL, BaseURL+"/models?client_version=") {
		t.Fatal("models use the wrong account API")
	}
}

func TestBrowserLoginFallsBackWithoutCancellingOtherApplications(t *testing.T) {
	f := newOAuthFixture(t)
	busy, err := net.Listen("tcp", "localhost:0")
	if err != nil {
		t.Fatal(err)
	}
	defer busy.Close()
	port := busy.Addr().(*net.TCPAddr).Port
	f.c.callbackPorts = []int{port, 0}
	login, q := f.start("")
	u, _ := url.Parse(q.Get("redirect_uri"))
	if u.Port() == fmt.Sprint(port) {
		t.Fatal("reused another application's listener")
	}
	f.c.CancelLogin(login.LoginID)
}

func newOAuthFixture(t *testing.T) *oauthFixture {
	t.Helper()
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	f := &oauthFixture{t: t, key: key, subject: "user-one", workspace: "workspace-one"}
	f.c = New(t.TempDir())
	server := httptest.NewServer(http.HandlerFunc(f.serve))
	t.Cleanup(server.Close)
	t.Cleanup(f.c.Close)
	f.c.issuer = server.URL
	f.c.authorizeURL, f.c.tokenURL = server.URL+"/oauth/authorize", server.URL+"/oauth/token"
	f.c.jwksURL, f.c.modelsURL = server.URL+"/jwks", server.URL+"/models?client_version="+protocolVersion
	f.c.deviceCodeURL, f.c.deviceTokenURL = server.URL+"/deviceauth/usercode", server.URL+"/deviceauth/token"
	f.c.verificationURL = server.URL + "/codex/device"
	f.c.callbackPorts, f.c.pollingMargin = []int{0}, 0
	return f
}

func (f *oauthFixture) sign(claims map[string]any) string {
	f.t.Helper()
	encode := func(v any) string { raw, _ := json.Marshal(v); return base64.RawURLEncoding.EncodeToString(raw) }
	payload := encode(map[string]string{"alg": "RS256", "kid": "test-key"}) + "." + encode(claims)
	sum := sha256.Sum256([]byte(payload))
	signature, err := rsa.SignPKCS1v15(rand.Reader, f.key, crypto.SHA256, sum[:])
	if err != nil {
		f.t.Fatal(err)
	}
	return payload + "." + base64.RawURLEncoding.EncodeToString(signature)
}

func (f *oauthFixture) serve(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	defer f.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	switch r.URL.Path {
	case "/jwks":
		_ = json.NewEncoder(w).Encode(map[string]any{"keys": []any{map[string]string{
			"kid": "test-key", "kty": "RSA", "alg": "RS256", "use": "sig",
			"n": base64.RawURLEncoding.EncodeToString(f.key.PublicKey.N.Bytes()), "e": "AQAB",
		}}})
	case "/oauth/token":
		_ = r.ParseForm()
		if r.Form.Get("client_id") != OAuthClientID || r.Form.Has("resource") {
			f.t.Error("token exchange did not use the shared Codex client")
		}
		if r.Form.Get("grant_type") == "refresh_token" {
			f.refreshes.Add(1)
			if r.Form.Get("refresh_token") != "refresh-one" {
				f.t.Error("refresh token was not serialized")
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "access-two", "refresh_token": "refresh-two", "expires_in": 3600})
			return
		}
		f.exchanges.Add(1)
		if r.Form.Get("code") == "device-code" {
			if r.Form.Get("code_verifier") != "device-verifier" || r.Form.Get("redirect_uri") != f.c.issuer+"/deviceauth/callback" {
				f.t.Error("wrong device code exchange")
			}
		} else {
			sum := sha256.Sum256([]byte(r.Form.Get("code_verifier")))
			if base64.RawURLEncoding.EncodeToString(sum[:]) != f.challenge || r.Form.Get("redirect_uri") != f.redirect {
				f.t.Error("PKCE or callback URI did not match")
			}
		}
		claims := map[string]any{"iss": f.c.issuer, "aud": OAuthClientID, "sub": f.subject, "email": "person@example.com", "exp": time.Now().Add(time.Hour).Unix(), "https://api.openai.com/auth": map[string]string{"chatgpt_account_id": f.workspace}}
		switch f.tokenFault {
		case "issuer":
			claims["iss"] = "https://wrong.invalid"
		case "audience":
			claims["aud"] = "wrong-client"
		case "expired":
			claims["exp"] = time.Now().Add(-time.Minute).Unix()
		case "account":
			delete(claims, "https://api.openai.com/auth")
		}
		idToken := f.sign(claims)
		if f.tokenFault == "signature" {
			idToken += "broken"
		}
		// Codex OAuth responses need not contain token_type, scope, or expires_in.
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "access-one", "refresh_token": "refresh-one", "id_token": idToken})
	case "/deviceauth/usercode":
		var body map[string]string
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["client_id"] != OAuthClientID {
			f.t.Error("device login used an agent client")
		}
		f.deviceStarts.Add(1)
		_, _ = w.Write([]byte(`{"device_auth_id":"private-device-id","user_code":"ABCD-1234","interval":"1"}`))
	case "/deviceauth/token":
		var body map[string]string
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["device_auth_id"] != "private-device-id" || body["user_code"] != "ABCD-1234" {
			f.t.Error("wrong device polling body")
		}
		n := int(f.polls.Add(1))
		if f.deviceFailure != 0 {
			w.WriteHeader(f.deviceFailure)
			return
		}
		if n <= len(f.pendingStatuses) {
			w.WriteHeader(f.pendingStatuses[n-1])
			return
		}
		_, _ = w.Write([]byte(`{"authorization_code":"device-code","code_verifier":"device-verifier"}`))
	case "/models":
		if !strings.HasPrefix(r.Header.Get("Authorization"), "Bearer access-") || r.Header.Get("ChatGPT-Account-Id") != f.workspace || r.Header.Get("Originator") != "recur" || r.URL.Query().Get("client_version") == "" {
			f.t.Error("model request missing Codex account routing")
		}
		_, _ = w.Write([]byte(`{"models":[{"slug":"hidden","visibility":"hide"},{"slug":"gpt-first","display_name":"First","visibility":"list"},{"slug":"gpt-second","display_name":"Second","visibility":"list"}]}`))
	default:
		http.NotFound(w, r)
	}
}

func (f *oauthFixture) start(account string) (Login, url.Values) {
	f.t.Helper()
	login, err := f.c.StartLogin(account, LoginBrowser)
	if err != nil {
		f.t.Fatal(err)
	}
	u, err := url.Parse(login.AuthURL)
	if err != nil {
		f.t.Fatal(err)
	}
	q := u.Query()
	f.mu.Lock()
	f.challenge, f.redirect = q.Get("code_challenge"), q.Get("redirect_uri")
	f.mu.Unlock()
	return login, q
}

func (f *oauthFixture) callback(q url.Values, state string) int {
	f.t.Helper()
	u, _ := url.Parse(q.Get("redirect_uri"))
	u.RawQuery = url.Values{"state": {state}, "code": {"test-code"}}.Encode()
	resp, err := http.Get(u.String())
	if err != nil {
		f.t.Fatal(err)
	}
	defer resp.Body.Close()
	return resp.StatusCode
}

func (f *oauthFixture) connect() string {
	f.t.Helper()
	login, q := f.start("")
	f.callback(q, q.Get("state"))
	if err := f.c.WaitLogin(login.LoginID); err != nil {
		f.t.Fatal(err)
	}
	status, _ := f.c.Status()
	return status.AccountID
}

func TestBrowserLoginUsesCodexFlowWithoutAgentRegistration(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	f := newOAuthFixture(t)
	login, q := f.start("")
	if q.Get("client_id") != OAuthClientID || q.Get("scope") != "openid profile email offline_access" || q.Get("originator") != "recur" || q.Get("codex_cli_simplified_flow") != "true" || q.Get("id_token_add_organizations") != "true" {
		t.Fatal("wrong Codex browser flow")
	}
	for _, key := range []string{"agent_name_hint", "ext_agent_host_id", "resource"} {
		if q.Has(key) {
			t.Fatalf("agent-registration parameter %s was sent", key)
		}
	}
	redirect, _ := url.Parse(q.Get("redirect_uri"))
	if redirect.Hostname() != "localhost" || redirect.Path != "/auth/callback" {
		t.Fatal("incorrect Codex callback URI")
	}
	if f.callback(q, "wrong-state") != http.StatusBadRequest || f.exchanges.Load() != 0 {
		t.Fatal("invalid state reached the token exchange")
	}
	f.callback(q, q.Get("state"))
	if err := f.c.WaitLogin(login.LoginID); err != nil {
		t.Fatal(err)
	}
	status, err := f.c.Status()
	if err != nil || !status.Authenticated || status.AccountID == "" {
		t.Fatalf("status = %+v, %v", status, err)
	}
	public, _ := json.Marshal(status)
	if strings.Contains(string(public), "access-one") || strings.Contains(string(public), "refresh-one") {
		t.Fatal("tokens leaked into status")
	}
	stat, err := os.Stat(filepath.Join(f.c.dir, "chatgpt-auth.json"))
	if err != nil || stat.Mode().Perm() != 0o600 {
		t.Fatalf("credential permissions: %v, %v", stat, err)
	}
	models, err := f.c.Models(t.Context(), status.AccountID)
	if err != nil || len(models) != 2 || models[0].ID != "gpt-first" {
		t.Fatalf("models = %+v, %v", models, err)
	}
	token, err := New(f.c.dir).AccessToken(t.Context(), status.AccountID)
	if err != nil || token != "access-one" {
		t.Fatalf("restart token: %v", err)
	}
	login, returning := f.start(status.AccountID)
	if returning.Get("client_id") != OAuthClientID || returning.Get("login_hint") != "person@example.com" || returning.Get("state") == q.Get("state") || returning.Get("code_challenge") == q.Get("code_challenge") {
		t.Fatal("returning login reused secrets or registered an agent")
	}
	f.c.CancelLogin(login.LoginID)
}

func TestHeadlessLoginPollsAndExchangesWithoutListener(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	f := newOAuthFixture(t)
	f.pendingStatuses = []int{http.StatusForbidden, http.StatusNotFound}
	f.c.callbackPorts = nil // Any attempt to bind a browser listener would fail.
	login, err := f.c.StartLogin("", LoginHeadless)
	if err != nil {
		t.Fatal(err)
	}
	if login.AuthURL != f.c.verificationURL || login.UserCode != "ABCD-1234" || login.Method != LoginHeadless {
		t.Fatal("device link/code not returned")
	}
	public, _ := json.Marshal(login)
	if strings.Contains(string(public), "private-device-id") {
		t.Fatal("private device ID leaked into settings")
	}
	if err := f.c.WaitLogin(login.LoginID); err != nil {
		t.Fatal(err)
	}
	status, _ := f.c.Status()
	if !status.Authenticated || f.polls.Load() != 3 || f.exchanges.Load() != 1 {
		t.Fatalf("device flow did not complete: %+v", status)
	}
}

func TestLoginValidatesIdentity(t *testing.T) {
	for _, fault := range []string{"signature", "issuer", "audience", "expired", "account"} {
		t.Run(fault, func(t *testing.T) {
			f := newOAuthFixture(t)
			f.tokenFault = fault
			login, q := f.start("")
			f.callback(q, q.Get("state"))
			if err := f.c.WaitLogin(login.LoginID); err == nil {
				t.Fatal("invalid identity was accepted")
			}
			status, _ := f.c.Status()
			if status.Authenticated {
				t.Fatal("failed login activated credentials")
			}
		})
	}
}

func TestRefreshIsSerializedAndRotated(t *testing.T) {
	f := newOAuthFixture(t)
	id := f.connect()
	s, _ := f.c.read()
	s.account(id).ExpiresAt = time.Now().Add(-time.Second)
	if err := f.c.write(s); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	for range 8 {
		wg.Go(func() {
			token, err := f.c.AccessToken(context.Background(), id)
			if err != nil || token != "access-two" {
				t.Errorf("refresh failed: %v", err)
			}
		})
	}
	wg.Wait()
	if f.refreshes.Load() != 1 {
		t.Fatalf("refreshed %d times", f.refreshes.Load())
	}
	s, _ = f.c.read()
	if s.account(id).RefreshToken != "refresh-two" {
		t.Fatal("rotated token was not persisted")
	}
	if err := f.c.Disconnect(); err != nil {
		t.Fatal(err)
	}
	status, _ := f.c.Status()
	if status.Authenticated {
		t.Fatal("sign-out did not clear credentials")
	}
}

func TestCancelledAndForeignAccountLoginPreserveActiveAccount(t *testing.T) {
	f := newOAuthFixture(t)
	id := f.connect()
	login, _ := f.start("")
	f.c.CancelLogin(login.LoginID)
	if err := f.c.WaitLogin(login.LoginID); err == nil {
		t.Fatal("cancelled attempt succeeded")
	}
	login, q := f.start(id)
	f.mu.Lock()
	f.subject = "a-different-user"
	f.mu.Unlock()
	f.callback(q, q.Get("state"))
	if err := f.c.WaitLogin(login.LoginID); err == nil {
		t.Fatal("changed account identity was accepted")
	}
	if token, err := f.c.AccessToken(t.Context(), id); err != nil || token != "access-one" {
		t.Fatal("failed switch changed the active session")
	}
}

func TestDeviceFailureAndCancellationKeepActiveAccount(t *testing.T) {
	for _, cancel := range []bool{false, true} {
		t.Run(fmt.Sprint(cancel), func(t *testing.T) {
			f := newOAuthFixture(t)
			id := f.connect()
			if cancel {
				f.pendingStatuses = []int{http.StatusForbidden}
			} else {
				f.deviceFailure = http.StatusBadRequest
			}
			login, err := f.c.StartLogin("", LoginHeadless)
			if err != nil {
				t.Fatal(err)
			}
			if cancel {
				f.c.CancelLogin(login.LoginID)
			}
			if err := f.c.WaitLogin(login.LoginID); err == nil {
				t.Fatal("failed device attempt succeeded")
			}
			status, _ := f.c.Status()
			if !status.Authenticated || status.AccountID != id {
				t.Fatal("device failure replaced the active account")
			}
		})
	}
}

func TestLoginCancellationClosesListener(t *testing.T) {
	f := newOAuthFixture(t)
	login, q := f.start("")
	f.c.CancelLogin(login.LoginID)
	client := &http.Client{Timeout: time.Second}
	if resp, err := client.Get(q.Get("redirect_uri")); err == nil {
		resp.Body.Close()
		t.Fatal("cancelled listener is still accepting callbacks")
	}
}

func TestDevicePollingTimeout(t *testing.T) {
	f := newOAuthFixture(t)
	f.pendingStatuses = []int{http.StatusForbidden}
	ctx, cancel := context.WithTimeout(t.Context(), 50*time.Millisecond)
	defer cancel()
	_, err := f.c.pollDevice(ctx, deviceCode{AuthID: "private-device-id", UserCode: "ABCD-1234", Interval: json.RawMessage(`"1"`)})
	if err == nil {
		t.Fatal("polling did not respect timeout")
	}
}

func TestAgentCredentialsAreNotReusedForCodex(t *testing.T) {
	f := newOAuthFixture(t)
	if err := f.c.write(authFile{Active: "old-agent", Accounts: []credentials{{Account: Account{ID: "old-agent"}, AccessToken: "agent-token", RefreshToken: "agent-refresh", Scope: "chatgpt.tokens.use.direct"}}}); err != nil {
		t.Fatal(err)
	}
	status, _ := f.c.Status()
	if status.Authenticated || len(status.Accounts) != 0 {
		t.Fatal("old agent credentials appeared as a Codex login")
	}
	if _, err := f.c.AccessToken(t.Context(), "old-agent"); err == nil {
		t.Fatal("agent token was reused for Codex")
	}
}

func TestOAuthErrorsDoNotExposeTokenBodies(t *testing.T) {
	f := newOAuthFixture(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "secret-token-body", http.StatusUnauthorized)
	}))
	defer srv.Close()
	f.c.tokenURL = srv.URL
	_, err := f.c.exchange(t.Context(), url.Values{"client_id": {OAuthClientID}})
	if err == nil || strings.Contains(fmt.Sprint(err), "secret-token-body") {
		t.Fatal("token response leaked or error was ignored")
	}
}
