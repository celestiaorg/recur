package responses

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestClientStreamSSE(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/responses" {
			http.NotFound(w, r)
			return
		}
		if r.Header.Get("Authorization") != "Bearer sk-test" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = w.Write([]byte("data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n"))
		_, _ = w.Write([]byte("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"))
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, APIKey: "sk-test", HTTP: srv.Client()}
	var deltas []string
	if err := c.Stream(t.Context(), Request{Model: "gpt-5", Input: []InputItem{UserText("hi")}}, func(e Event) {
		if e.Delta != "" {
			deltas = append(deltas, e.Delta)
		}
	}); err != nil {
		t.Fatalf("Stream: %v", err)
	}
	if strings.Join(deltas, "") != "ok" {
		t.Errorf("Stream deltas = %v, want ok", deltas)
	}
}

func TestClientStreamJSONFallback(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "r2",
			"status": "completed",
			"output": []any{map[string]any{
				"type":    "message",
				"content": []any{map[string]string{"type": "output_text", "text": "json"}},
			}},
		})
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	var text string
	if err := c.Stream(t.Context(), Request{Model: "m", Input: []InputItem{UserText("x")}}, func(e Event) {
		text += e.Delta
	}); err != nil {
		t.Fatalf("Stream: %v", err)
	}
	if text != "json" {
		t.Errorf("Stream JSON text = %q, want json", text)
	}
}

func TestClientStreamRejectsEmptyJSON(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{}`))
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Stream(t.Context(), Request{Model: "m", Input: []InputItem{UserText("x")}}, nil); err == nil {
		t.Fatal("Stream({}) error = nil, want decode error")
	}
}

func TestClientPingError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "nope", http.StatusUnauthorized)
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	ctx, cancel := t.Context(), func() {}
	_ = cancel
	if err := c.Ping(ctx, "gpt-5"); err == nil {
		t.Fatal("Ping error = nil, want unauthorized")
	}
	_ = time.Second
}

func TestClientPingRejectsFailedBody(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "r-fail",
			"status": "failed",
			"error":  map[string]string{"message": "model not found"},
		})
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Ping(t.Context(), "gpt-5"); err == nil || !strings.Contains(err.Error(), "model not found") {
		t.Fatalf("Ping failed body err = %v, want model not found", err)
	}
}

func TestClientPingRejectsHTML(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		_, _ = w.Write([]byte("<html>not json</html>"))
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Ping(t.Context(), "gpt-5"); err == nil {
		t.Fatal("Ping HTML error = nil, want decode error")
	}
}

func TestClientPingRejectsEmptyJSON(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{}`))
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Ping(t.Context(), "gpt-5"); err == nil {
		t.Fatal("Ping({}) error = nil, want not a Responses body")
	}
}

func TestClientPingRejectsQueuedStatus(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"id": "r", "status": "queued", "output": []any{}})
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Ping(t.Context(), "gpt-5"); err == nil {
		t.Fatal("Ping(queued) error = nil, want status error")
	}
}

func TestClientPingAcceptsIncomplete(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":     "r-inc",
			"status": "incomplete",
			"output": []any{},
		})
	}))
	t.Cleanup(srv.Close)
	c := &Client{BaseURL: srv.URL, HTTP: srv.Client()}
	if err := c.Ping(t.Context(), "gpt-5"); err != nil {
		t.Fatalf("Ping(incomplete) = %v, want nil", err)
	}
}

func TestStreamHTTPHasNoHardTimeout(t *testing.T) {
	if got := (&Client{}).streamHTTP().Timeout; got != 0 {
		t.Errorf("stream HTTP Timeout = %v, want 0 (turn ctx is the ceiling)", got)
	}
}

func TestStreamHTTPDisablesCompression(t *testing.T) {
	c := (&Client{}).streamHTTP()
	tr, ok := c.Transport.(*http.Transport)
	if !ok {
		t.Fatalf("streamHTTP().Transport type = %T, want *http.Transport", c.Transport)
	}
	if !tr.DisableCompression {
		t.Error("streamHTTP DisableCompression = false, want true (gzip buffers SSE tokens)")
	}
}

func TestEndpoint(t *testing.T) {
	c := &Client{BaseURL: "https://api.openai.com/"}
	if got := c.Endpoint(); got != "https://api.openai.com/v1/responses" {
		t.Errorf("Endpoint() = %q, want https://api.openai.com/v1/responses", got)
	}
}
