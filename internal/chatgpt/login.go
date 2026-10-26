package chatgpt

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const (
	LoginBrowser  = "browser"
	LoginHeadless = "headless"
)

type Login struct {
	LoginID  string `json:"loginId"`
	AuthURL  string `json:"authUrl"`
	Method   string `json:"method"`
	UserCode string `json:"userCode,omitempty"`
}

type loginAttempt struct {
	Login
	ctx    context.Context
	cancel context.CancelFunc
	done   chan struct{}
	err    error // written before done closes
}

func randomSecret() string {
	b := make([]byte, 32)
	_, _ = rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func (c *Client) StartLogin(accountID, method string) (Login, error) {
	if method == "" {
		method = LoginBrowser
	}
	if method != LoginBrowser && method != LoginHeadless {
		return Login{}, fmt.Errorf("choose browser or headless ChatGPT sign-in")
	}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return Login{}, fmt.Errorf("ChatGPT provider is shutting down")
	}
	if c.pending != nil {
		select {
		case <-c.pending.done:
		default:
			c.mu.Unlock()
			return Login{}, fmt.Errorf("ChatGPT sign-in is already in progress")
		}
	}
	s, err := c.read()
	if err != nil {
		c.mu.Unlock()
		return Login{}, err
	}
	var previous *credentials
	if accountID != "" {
		a := s.account(accountID)
		if a == nil || a.ClientID != OAuthClientID {
			c.mu.Unlock()
			return Login{}, fmt.Errorf("unknown saved ChatGPT account; start a new sign-in")
		}
		copy := *a
		previous = &copy
	}
	timeout := 5 * time.Minute
	if method == LoginHeadless {
		timeout = 15 * time.Minute
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	p := &loginAttempt{Login: Login{LoginID: randomSecret(), Method: method}, ctx: ctx, cancel: cancel, done: make(chan struct{})}
	c.pending = p
	c.mu.Unlock()
	if method == LoginHeadless {
		return c.startDeviceLogin(p, previous)
	}
	listener, redirect, err := c.listenBrowser()
	if err != nil {
		p.err = err
		p.cancel()
		close(p.done)
		return Login{}, err
	}
	state, verifier := randomSecret(), randomSecret()
	challenge := sha256.Sum256([]byte(verifier))
	u, err := url.Parse(c.authorizeURL)
	if err != nil {
		_ = listener.Close()
		p.err = err
		p.cancel()
		close(p.done)
		return Login{}, err
	}
	q := url.Values{
		"response_type": {"code"}, "client_id": {OAuthClientID}, "redirect_uri": {redirect},
		"scope":          {"openid profile email offline_access"},
		"code_challenge": {base64.RawURLEncoding.EncodeToString(challenge[:])}, "code_challenge_method": {"S256"},
		"id_token_add_organizations": {"true"}, "codex_cli_simplified_flow": {"true"},
		"state": {state}, "originator": {"recur"},
	}
	if previous != nil && previous.Email != "" {
		q.Set("login_hint", previous.Email)
	}
	u.RawQuery = q.Encode()
	p.AuthURL = u.String()
	go c.completeBrowserLogin(p, listener, redirect, state, verifier, previous)
	return p.Login, nil
}

func (c *Client) listenBrowser() (net.Listener, string, error) {
	for _, port := range c.callbackPorts {
		listener, err := net.Listen("tcp", net.JoinHostPort("localhost", strconv.Itoa(port)))
		if err != nil {
			continue
		}
		actual := listener.Addr().(*net.TCPAddr).Port
		return listener, "http://localhost:" + strconv.Itoa(actual) + "/auth/callback", nil
	}
	return nil, "", fmt.Errorf("ChatGPT browser sign-in needs an available localhost port (1455 or 1457). Close another pending login or choose headless sign-in")
}

func (c *Client) completeBrowserLogin(p *loginAttempt, listener net.Listener, redirect, state, verifier string, previous *credentials) {
	callback := make(chan url.Values, 1)
	server := &http.Server{ReadHeaderTimeout: 5 * time.Second}
	server.Handler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Referrer-Policy", "no-referrer")
		if r.Method != http.MethodGet || r.URL.Path != "/auth/callback" {
			http.NotFound(w, r)
			return
		}
		q := r.URL.Query()
		if q.Get("state") != state {
			http.Error(w, "Invalid sign-in state. Return to Recur and try again.", http.StatusBadRequest)
			return
		}
		select {
		case callback <- q:
		default:
		}
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		_, _ = w.Write([]byte("You can return to Recur. It will confirm whether ChatGPT sign-in completed."))
	})
	defer func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		_ = server.Shutdown(ctx)
		cancel()
		_ = listener.Close()
		p.cancel()
		close(p.done)
	}()
	serveErr := make(chan error, 1)
	go func() { serveErr <- server.Serve(listener) }()
	var q url.Values
	select {
	case <-p.ctx.Done():
		p.err = fmt.Errorf("ChatGPT sign-in cancelled or timed out")
		return
	case <-serveErr:
		p.err = fmt.Errorf("ChatGPT callback listener stopped")
		return
	case q = <-callback:
	}
	if q.Get("error") != "" || q.Get("code") == "" {
		p.err = fmt.Errorf("ChatGPT sign-in was declined or failed. Try signing in again")
		return
	}
	tokens, err := c.exchange(p.ctx, codeGrant(q.Get("code"), redirect, verifier))
	if err != nil {
		p.err = err
		return
	}
	p.err = c.saveLogin(p, tokens, previous)
}

func codeGrant(code, redirect, verifier string) url.Values {
	return url.Values{"grant_type": {"authorization_code"}, "code": {code}, "redirect_uri": {redirect}, "client_id": {OAuthClientID}, "code_verifier": {verifier}}
}

func credentialID(subject, workspace string) string {
	sum := sha256.Sum256([]byte(subject + "\x00" + workspace))
	return hex.EncodeToString(sum[:16])
}

func (c *Client) saveLogin(p *loginAttempt, tokens tokenResponse, previous *credentials) error {
	identity, err := c.validateIDToken(p.ctx, tokens.IDToken, OAuthClientID)
	if err != nil {
		return err
	}
	workspace := identity.accountID()
	if workspace == "" {
		// Account routing metadata may be supplied only in the access token.
		// This token came directly from the authenticated OAuth token endpoint.
		parts := strings.Split(tokens.AccessToken, ".")
		if len(parts) == 3 {
			var access identityClaims
			if raw, err := base64.RawURLEncoding.DecodeString(parts[1]); err == nil && json.Unmarshal(raw, &access) == nil {
				workspace = access.accountID()
			}
		}
	}
	if workspace == "" || tokens.RefreshToken == "" {
		return fmt.Errorf("ChatGPT did not return a renewable account session")
	}
	id := credentialID(identity.Subject, workspace)
	if previous != nil && (previous.ID != id || previous.Subject != identity.Subject) {
		return fmt.Errorf("ChatGPT returned a different account. Choose another ChatGPT account to switch")
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.pending != p || p.ctx.Err() != nil || c.closed {
		return fmt.Errorf("ChatGPT sign-in was cancelled")
	}
	s, err := c.read()
	if err != nil {
		return err
	}
	a := s.account(id)
	if a == nil {
		s.Accounts = append(s.Accounts, credentials{})
		a = &s.Accounts[len(s.Accounts)-1]
	}
	*a = credentials{
		Account: Account{ID: id, Email: identity.Email}, Subject: identity.Subject,
		ClientID: OAuthClientID, WorkspaceID: workspace,
		AccessToken: tokens.AccessToken, RefreshToken: tokens.RefreshToken, IDToken: tokens.IDToken, Scope: tokens.Scope,
		ExpiresAt: time.Now().Add(time.Duration(tokens.ExpiresIn) * time.Second),
	}
	s.Active = id
	return c.write(s)
}

func (c *Client) WaitLogin(loginID string) error {
	c.mu.Lock()
	p := c.pending
	c.mu.Unlock()
	if p == nil || p.LoginID != loginID {
		return fmt.Errorf("ChatGPT sign-in is no longer pending")
	}
	<-p.done
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.pending != p {
		return fmt.Errorf("ChatGPT sign-in was cancelled")
	}
	c.pending = nil
	return p.err
}

func (c *Client) CancelLogin(loginID string) {
	c.mu.Lock()
	p := c.pending
	if p == nil || p.LoginID != loginID {
		c.mu.Unlock()
		return
	}
	c.pending = nil
	p.cancel()
	c.mu.Unlock()
	<-p.done
}

func (c *Client) Close() {
	c.mu.Lock()
	c.closed = true
	p := c.pending
	c.pending = nil
	if p != nil {
		p.cancel()
	}
	c.mu.Unlock()
	if p != nil {
		<-p.done
	}
}
