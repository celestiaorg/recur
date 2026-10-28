package chatgpt

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"time"
)

type deviceCode struct {
	AuthID        string          `json:"device_auth_id"`
	UserCode      string          `json:"user_code"`
	UserCodeAlias string          `json:"usercode"`
	Interval      json.RawMessage `json:"interval"`
}

func (c *Client) startDeviceLogin(p *loginAttempt, previous *credentials) (Login, error) {
	fail := func(err error) (Login, error) { p.err = err; p.cancel(); close(p.done); return Login{}, err }
	req, err := jsonRequest(p.ctx, c.deviceCodeURL, map[string]string{"client_id": OAuthClientID})
	if err != nil {
		return fail(err)
	}
	raw, err := c.request(req)
	if err != nil {
		return fail(fmt.Errorf("start headless sign-in: %w", err))
	}
	var code deviceCode
	if json.Unmarshal(raw, &code) != nil {
		return fail(fmt.Errorf("invalid ChatGPT device authorization response"))
	}
	if code.UserCode == "" {
		code.UserCode = code.UserCodeAlias
	}
	if code.AuthID == "" || code.UserCode == "" {
		return fail(fmt.Errorf("ChatGPT did not return a device code"))
	}
	if p.ctx.Err() != nil {
		return fail(fmt.Errorf("ChatGPT sign-in was cancelled"))
	}
	p.AuthURL, p.UserCode = c.verificationURL, code.UserCode
	go func() {
		defer func() { p.cancel(); close(p.done) }()
		tokens, err := c.pollDevice(p.ctx, code)
		if err != nil {
			p.err = err
			return
		}
		p.err = c.saveLogin(p, tokens, previous)
	}()
	return p.Login, nil
}

func deviceInterval(raw json.RawMessage) time.Duration {
	var text string
	var seconds int
	if json.Unmarshal(raw, &text) == nil {
		seconds, _ = strconv.Atoi(text)
	} else {
		_ = json.Unmarshal(raw, &seconds)
	}
	if seconds == 0 {
		seconds = 5
	}
	if seconds < 1 {
		seconds = 1
	}
	return time.Duration(seconds) * time.Second
}

func jsonRequest(ctx context.Context, endpoint string, body any) (*http.Request, error) {
	raw, err := json.Marshal(body)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(raw))
	if err == nil {
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("User-Agent", "recur")
	}
	return req, err
}

func (c *Client) pollDevice(ctx context.Context, code deviceCode) (tokenResponse, error) {
	interval := deviceInterval(code.Interval) + c.pollingMargin
	for {
		req, err := jsonRequest(ctx, c.deviceTokenURL, map[string]string{"device_auth_id": code.AuthID, "user_code": code.UserCode})
		if err != nil {
			return tokenResponse{}, err
		}
		response, err := c.http.Do(req)
		if err != nil {
			return tokenResponse{}, fmt.Errorf("ChatGPT device sign-in cancelled, expired, or unavailable: %w", err)
		}
		if response.StatusCode >= 200 && response.StatusCode < 300 {
			var authorized struct {
				Code     string `json:"authorization_code"`
				Verifier string `json:"code_verifier"`
			}
			err := json.NewDecoder(io.LimitReader(response.Body, 1<<20)).Decode(&authorized)
			_ = response.Body.Close()
			if err != nil || authorized.Code == "" || authorized.Verifier == "" {
				return tokenResponse{}, fmt.Errorf("invalid ChatGPT device authorization result")
			}
			return c.exchange(ctx, codeGrant(authorized.Code, c.issuer+"/deviceauth/callback", authorized.Verifier))
		}
		_ = response.Body.Close()
		if response.StatusCode != http.StatusForbidden && response.StatusCode != http.StatusNotFound {
			return tokenResponse{}, fmt.Errorf("ChatGPT device authorization failed (HTTP %d). Start a new sign-in", response.StatusCode)
		}
		timer := time.NewTimer(interval)
		select {
		case <-ctx.Done():
			timer.Stop()
			return tokenResponse{}, fmt.Errorf("ChatGPT device sign-in cancelled or timed out")
		case <-timer.C:
		}
	}
}
