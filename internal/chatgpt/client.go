// Package chatgpt implements Codex-compatible browser and device-code OAuth
// without invoking the Codex CLI or reading another application's credentials.
package chatgpt

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// OAuthClientID is the public Codex client also used by OpenCode. It is not a
// secret, and does not dynamically register Recur as a ChatGPT agent.
const OAuthClientID = "app_EMoamEEZ73f0CkXaXp7hrann"

const BaseURL = "https://chatgpt.com/backend-api/codex"
const protocolVersion = "0.163.0"

// Client is shared by Settings and the engine, serializing rotating-token refresh.
type Client struct {
	mu                                                 sync.Mutex
	dir                                                string
	http                                               *http.Client
	issuer, authorizeURL, tokenURL, jwksURL, modelsURL string
	deviceCodeURL, deviceTokenURL, verificationURL     string
	callbackPorts                                      []int
	pollingMargin                                      time.Duration
	pending                                            *loginAttempt
	closed                                             bool
}

func New(dataDir string) *Client {
	return &Client{
		dir: dataDir, http: &http.Client{Timeout: 30 * time.Second},
		issuer:          "https://auth.openai.com",
		authorizeURL:    "https://auth.openai.com/oauth/authorize",
		tokenURL:        "https://auth.openai.com/oauth/token",
		jwksURL:         "https://auth.openai.com/.well-known/jwks.json",
		modelsURL:       BaseURL + "/models?client_version=" + protocolVersion,
		deviceCodeURL:   "https://auth.openai.com/api/accounts/deviceauth/usercode",
		deviceTokenURL:  "https://auth.openai.com/api/accounts/deviceauth/token",
		verificationURL: "https://auth.openai.com/codex/device",
		callbackPorts:   []int{1455, 1457}, pollingMargin: 3 * time.Second,
	}
}

// Account contains only identity metadata suitable for the settings UI.
type Account struct {
	ID    string `json:"id"`
	Email string `json:"email"`
}

type Status struct {
	Authenticated bool      `json:"authenticated"`
	AccountID     string    `json:"accountId"`
	Email         string    `json:"email"`
	Accounts      []Account `json:"accounts"`
}

type credentials struct {
	Account
	Subject      string    `json:"subject"`
	ClientID     string    `json:"clientId,omitempty"`
	WorkspaceID  string    `json:"chatgptAccountId,omitempty"`
	AccessToken  string    `json:"accessToken,omitempty"`
	RefreshToken string    `json:"refreshToken,omitempty"`
	IDToken      string    `json:"idToken,omitempty"`
	Scope        string    `json:"scope,omitempty"`
	ExpiresAt    time.Time `json:"expiresAt"`
}

type authFile struct {
	Active   string        `json:"active"`
	Accounts []credentials `json:"accounts"`
}

func (s *authFile) account(id string) *credentials {
	for i := range s.Accounts {
		if s.Accounts[i].ID == id {
			return &s.Accounts[i]
		}
	}
	return nil
}

func (c *Client) read() (authFile, error) {
	var out authFile
	raw, err := os.ReadFile(filepath.Join(c.dir, "chatgpt-auth.json"))
	if os.IsNotExist(err) {
		return out, nil
	}
	if err != nil {
		return out, err
	}
	if err := json.Unmarshal(raw, &out); err != nil {
		return out, fmt.Errorf("read ChatGPT credentials: %w", err)
	}
	return out, nil
}

func (c *Client) write(s authFile) error {
	if err := os.MkdirAll(c.dir, 0o700); err != nil {
		return err
	}
	raw, err := json.Marshal(s)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(c.dir, ".chatgpt-auth-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(raw); err != nil {
		_ = f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), filepath.Join(c.dir, "chatgpt-auth.json"))
}

func (c *Client) Status() (Status, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	s, err := c.read()
	if err != nil {
		return Status{}, err
	}
	out := Status{Accounts: []Account{}}
	for _, a := range s.Accounts {
		if a.ClientID == OAuthClientID {
			out.Accounts = append(out.Accounts, a.Account)
		}
	}
	if a := s.account(s.Active); a != nil && a.ClientID == OAuthClientID {
		out.AccountID, out.Email = a.ID, a.Email
		out.Authenticated = a.AccessToken != "" && a.WorkspaceID != ""
	}
	return out, nil
}

type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	IDToken      string `json:"id_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int64  `json:"expires_in"`
	Scope        string `json:"scope"`
}

func (c *Client) exchange(ctx context.Context, form url.Values) (tokenResponse, error) {
	var out tokenResponse
	raw, err := c.postForm(ctx, c.tokenURL, form)
	if err != nil {
		return out, err
	}
	if err := json.Unmarshal(raw, &out); err != nil {
		return out, fmt.Errorf("invalid ChatGPT token response")
	}
	if out.AccessToken == "" || (out.TokenType != "" && !strings.EqualFold(out.TokenType, "Bearer")) {
		return out, fmt.Errorf("incomplete ChatGPT token response")
	}
	if out.ExpiresIn <= 0 {
		out.ExpiresIn = 3600
	}
	return out, nil
}

func (c *Client) postForm(ctx context.Context, endpoint string, form url.Values) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, strings.NewReader(form.Encode()))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	return c.request(req)
}

func (c *Client) request(req *http.Request) ([]byte, error) {
	req.Header.Set("User-Agent", "recur")
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("could not reach OpenAI: %w", err)
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		// Error bodies can contain credentials or authorization URLs. Report
		// only the status, never echo those bodies into logs or the renderer.
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusBadRequest {
			return nil, fmt.Errorf("OpenAI rejected the session (HTTP %d). Continue with ChatGPT in Settings to sign in again", resp.StatusCode)
		}
		return nil, fmt.Errorf("OpenAI request failed (HTTP %d)", resp.StatusCode)
	}
	return raw, nil
}

// AccessToken refreshes near expiry. The expected account prevents a running
// turn from silently switching identities after the user changes accounts.
func (c *Client) AccessToken(ctx context.Context, accountID string) (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	a, err := c.accessCredentials(ctx, accountID)
	if err != nil {
		return "", err
	}
	return a.AccessToken, nil
}

// Authorize binds both bearer token and ChatGPT workspace to one credential
// snapshot. A running turn cannot pick up a different account after a switch.
func (c *Client) Authorize(req *http.Request, accountID string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	a, err := c.accessCredentials(req.Context(), accountID)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+a.AccessToken)
	req.Header.Set("ChatGPT-Account-Id", a.WorkspaceID)
	req.Header.Set("Originator", "recur")
	req.Header.Set("User-Agent", "recur")
	return nil
}

// Caller holds mu throughout refresh and persistence.
func (c *Client) accessCredentials(ctx context.Context, accountID string) (credentials, error) {
	if c.closed {
		return credentials{}, fmt.Errorf("ChatGPT provider is shutting down")
	}
	s, err := c.read()
	if err != nil {
		return credentials{}, err
	}
	a := s.account(accountID)
	if accountID == "" || s.Active != accountID || a == nil || a.AccessToken == "" || a.ClientID != OAuthClientID || a.WorkspaceID == "" {
		return credentials{}, fmt.Errorf("sign in using browser or headless login in Settings → Providers → OpenAI")
	}
	if time.Now().Add(time.Minute).Before(a.ExpiresAt) {
		return *a, nil
	}
	if a.RefreshToken == "" {
		return credentials{}, fmt.Errorf("ChatGPT session expired. Continue with ChatGPT in Settings → Providers → OpenAI")
	}
	tokens, err := c.exchange(ctx, url.Values{
		"grant_type": {"refresh_token"}, "client_id": {OAuthClientID},
		"refresh_token": {a.RefreshToken},
	})
	if err != nil {
		return credentials{}, err
	}
	if tokens.Scope == "" {
		tokens.Scope = a.Scope
	}
	if tokens.RefreshToken == "" {
		tokens.RefreshToken = a.RefreshToken
	}
	if tokens.IDToken != "" {
		identity, err := c.validateIDToken(ctx, tokens.IDToken, OAuthClientID)
		if err != nil {
			return credentials{}, err
		}
		if identity.Subject != a.Subject || (identity.accountID() != "" && identity.accountID() != a.WorkspaceID) {
			return credentials{}, fmt.Errorf("ChatGPT refresh returned a different account")
		}
		a.IDToken = tokens.IDToken
	}
	a.AccessToken, a.RefreshToken, a.Scope = tokens.AccessToken, tokens.RefreshToken, tokens.Scope
	a.ExpiresAt = time.Now().Add(time.Duration(tokens.ExpiresIn) * time.Second)
	if err := c.write(s); err != nil {
		return credentials{}, err
	}
	return *a, nil
}

// Disconnect removes Recur's active local credentials, like OpenCode sign-out.
func (c *Client) Disconnect() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	s, err := c.read()
	if err != nil {
		return err
	}
	if a := s.account(s.Active); a != nil {
		a.AccessToken, a.RefreshToken, a.IDToken, a.Scope = "", "", "", ""
		a.ExpiresAt = time.Time{}
	}
	s.Active = ""
	return c.write(s)
}

type Model struct {
	ID         string `json:"slug"`
	Name       string `json:"display_name"`
	Visibility string `json:"visibility"`
}

func (c *Client) Models(ctx context.Context, accountID string) ([]Model, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.modelsURL, nil)
	if err != nil {
		return nil, err
	}
	if err := c.Authorize(req, accountID); err != nil {
		return nil, err
	}
	raw, err := c.request(req)
	if err != nil {
		return nil, err
	}
	var out struct {
		Models []Model `json:"models"`
	}
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil, fmt.Errorf("invalid ChatGPT model catalogue")
	}
	var models []Model
	for _, m := range out.Models {
		if m.Visibility == "list" && strings.TrimSpace(m.ID) != "" {
			if m.Name == "" {
				m.Name = m.ID
			}
			models = append(models, m)
		}
	}
	if len(models) == 0 {
		return nil, fmt.Errorf("no models are available for this ChatGPT account")
	}
	return models, nil
}
