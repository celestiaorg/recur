package responses

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"recur/internal/domain"
)

// Client talks to an OpenAI-compatible POST {base}/v1/responses endpoint.
type Client struct {
	BaseURL      string
	APIKey       string
	ExtraHeaders map[string]string
	HTTP         *http.Client
	// Token is evaluated on every request so rotating OAuth tokens are never
	// frozen into a runner. ChatGPT enables the Codex subscription contract.
	Token     func(context.Context) (string, error)
	ChatGPT   bool
	Authorize func(*http.Request) error
}

// Endpoint returns the Responses URL for this client.
func (c *Client) Endpoint() string {
	if c.ChatGPT {
		return strings.TrimRight(strings.TrimSpace(c.BaseURL), "/") + "/responses"
	}
	return strings.TrimRight(strings.TrimSpace(c.BaseURL), "/") + "/v1/responses"
}

// Stream POSTs req with stream:true and invokes emit for each parsed event.
// If the server returns JSON instead of SSE, DecodeResponse events are emitted.
func (c *Client) Stream(ctx context.Context, req Request, emit func(Event)) error {
	req.Stream = true
	req.Store = false
	body, err := json.Marshal(req)
	if err != nil {
		return fmt.Errorf("responses encode: %w", err)
	}
	return c.do(ctx, body, true, emit)
}

func (c *Client) do(ctx context.Context, body []byte, stream bool, emit func(Event)) error {
	if c.ChatGPT {
		var err error
		body, err = chatGPTBody(body)
		if err != nil {
			return err
		}
		stream = true
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, c.Endpoint(), bytes.NewReader(body))
	if err != nil {
		return err
	}
	httpReq.Header.Set("Content-Type", "application/json")
	if stream {
		httpReq.Header.Set("Accept", "text/event-stream")
	}
	if err := c.applyAuth(httpReq); err != nil {
		return err
	}

	client := c.streamHTTP()
	resp, err := client.Do(httpReq)
	if err != nil {
		return fmt.Errorf("responses POST: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		raw, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		msg := strings.TrimSpace(string(raw))
		if msg == "" {
			msg = resp.Status
		}
		return fmt.Errorf("responses %s: %s", resp.Status, clipErr(msg))
	}
	ct := resp.Header.Get("Content-Type")
	if strings.Contains(ct, "text/event-stream") {
		return ParseSSE(resp.Body, emit)
	}
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 32<<20))
	if err != nil {
		return fmt.Errorf("responses read: %w", err)
	}
	if looksLikeSSE(raw) {
		return ParseSSE(bytes.NewReader(raw), emit)
	}
	evs, err := DecodeResponse(raw)
	if err != nil {
		return fmt.Errorf("responses decode: %w", err)
	}
	for _, ev := range evs {
		if emit != nil {
			emit(ev)
		}
	}
	return nil
}

// StreamJSON POSTs a pre-built JSON body (after flag wiring) as a stream.
func (c *Client) StreamJSON(ctx context.Context, body []byte, emit func(Event)) error {
	return c.do(ctx, body, true, emit)
}

// Ping sends a tiny non-tool Responses request to verify the endpoint.
func (c *Client) Ping(ctx context.Context, model string) error {
	model = strings.TrimSpace(model)
	if model == "" {
		return fmt.Errorf("model is required")
	}
	maxTok := 16
	req := Request{
		Model:           model,
		Input:           []InputItem{UserText("ping")},
		MaxOutputTokens: maxTok,
		Store:           false,
		Stream:          false,
	}
	body, err := json.Marshal(req)
	if err != nil {
		return err
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, c.Endpoint(), bytes.NewReader(body))
	if err != nil {
		return err
	}
	httpReq.Header.Set("Content-Type", "application/json")
	if err := c.applyAuth(httpReq); err != nil {
		return err
	}
	client := c.HTTP
	if client == nil {
		client = &http.Client{Timeout: 30 * time.Second}
	}
	resp, err := client.Do(httpReq)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		msg := strings.TrimSpace(string(raw))
		if msg == "" {
			msg = resp.Status
		}
		return fmt.Errorf("%s: %s", resp.Status, clipErr(msg))
	}
	evs, err := DecodeResponse(raw)
	if err != nil {
		return fmt.Errorf("not a Responses body: %s", clipErr(string(raw)))
	}
	var terminal bool
	for _, ev := range evs {
		if ev.ErrorMessage != "" {
			return fmt.Errorf("%s", ev.ErrorMessage)
		}
		if ev.Type == "error" || ev.Type == "response.failed" {
			return fmt.Errorf("provider failed")
		}
		if ev.Status == "failed" || ev.Status == "cancelled" || ev.Status == "canceled" {
			return fmt.Errorf("responses status %s", ev.Status)
		}
		// Token-capped probes often come back incomplete; that still proves the
		// endpoint speaks Responses. The harness rejects incomplete mid-turn.
		if ev.Type == "response.completed" || ev.Type == "response.incomplete" || ev.Status == "completed" || ev.Status == "incomplete" {
			terminal = true
		}
	}
	if !terminal {
		return fmt.Errorf("not a Responses body: %s", clipErr(string(raw)))
	}
	return nil
}

func (c *Client) applyAuth(req *http.Request) error {
	k := strings.TrimSpace(c.APIKey)
	if c.Token != nil {
		var err error
		k, err = c.Token(req.Context())
		if err != nil {
			return err
		}
	}
	if k != "" {
		req.Header.Set("Authorization", "Bearer "+k)
	}
	for k, v := range c.ExtraHeaders {
		if k == "" {
			continue
		}
		req.Header.Set(k, v)
	}
	if c.Authorize != nil {
		return c.Authorize(req)
	}
	return nil
}

func looksLikeSSE(raw []byte) bool {
	s := strings.TrimSpace(string(raw))
	return strings.HasPrefix(s, "event:") || strings.HasPrefix(s, "data:")
}

func clipErr(s string) string {
	if len(s) > 800 {
		return s[:800] + "…"
	}
	return s
}

// streamHTTP is the client for /v1/responses streams. No Timeout: the turn
// context (Settings.AgentTurnTimeout) is the ceiling. A 10m Client.Timeout
// killed local models that generate slower than that.
func (c *Client) streamHTTP() *http.Client {
	if c != nil && c.HTTP != nil {
		return c.HTTP
	}
	return &http.Client{Transport: streamTransport()}
}

// streamTransport is the RoundTripper for SSE. DisableCompression is required:
// the default Transport sends Accept-Encoding: gzip, and a gzipped event-stream
// only surfaces tokens when the compressor flushes — the chat looks frozen
// while the model is still generating.
func streamTransport() *http.Transport {
	if t, ok := http.DefaultTransport.(*http.Transport); ok {
		t = t.Clone()
		t.DisableCompression = true
		return t
	}
	return &http.Transport{Proxy: http.ProxyFromEnvironment, DisableCompression: true}
}

// NewClient builds a Client from a compatible provider config.
func NewClient(p domain.CompatibleProvider) *Client {
	return &Client{
		BaseURL:      p.BaseURL,
		APIKey:       p.APIKey,
		ExtraHeaders: p.ExtraHeaders,
	}
}
