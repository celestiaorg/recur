package codex

import (
	"context"
	"encoding/json"
	"io"
	"testing"
	"time"
)

func TestConnCallAndNotification(t *testing.T) {
	pr, pw := io.Pipe()
	cr, cw := io.Pipe()
	// Server side writes responses into cr's writer (cw); client reads cr.
	// Client writes requests into pw; server reads pr.
	conn := newConn(cr, pw)

	go func() {
		dec := json.NewDecoder(pr)
		for {
			var msg map[string]json.RawMessage
			if err := dec.Decode(&msg); err != nil {
				return
			}
			var method string
			_ = json.Unmarshal(msg["method"], &method)
			if method == "initialize" {
				resp := map[string]any{
					"id":     jsonRaw(msg["id"]),
					"result": map[string]any{"ok": true},
				}
				b, _ := json.Marshal(resp)
				_, _ = cw.Write(append(b, '\n'))
				// Also emit a notification.
				n, _ := json.Marshal(map[string]any{
					"method": "turn/completed",
					"params": map[string]any{"turn": map[string]any{"id": "t1", "status": "completed"}},
				})
				_, _ = cw.Write(append(n, '\n'))
			}
		}
	}()

	gotNotify := make(chan string, 1)
	conn.OnNotification(func(method string, _ json.RawMessage) {
		gotNotify <- method
	})

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	var out map[string]any
	if err := conn.Call(ctx, "initialize", map[string]any{"x": 1}, &out); err != nil {
		t.Fatalf("Call: %v", err)
	}
	if out["ok"] != true {
		t.Fatalf("result=%v", out)
	}
	select {
	case m := <-gotNotify:
		if m != "turn/completed" {
			t.Fatalf("notify=%q", m)
		}
	case <-ctx.Done():
		t.Fatal("no notification")
	}
}

func TestConnServerRequestAutoAnswer(t *testing.T) {
	pr, pw := io.Pipe()
	cr, cw := io.Pipe()
	conn := newConn(cr, pw)
	conn.OnServerRequest(AutoApproveServerRequest)

	answered := make(chan map[string]any, 1)
	go func() {
		dec := json.NewDecoder(pr)
		for {
			var msg map[string]json.RawMessage
			if err := dec.Decode(&msg); err != nil {
				return
			}
			// Client answering a server request — has id+result, no method.
			if _, ok := msg["method"]; !ok {
				var result map[string]any
				_ = json.Unmarshal(msg["result"], &result)
				answered <- result
				return
			}
		}
	}()

	// Inject a server→client elicitation request on the client's reader.
	req, _ := json.Marshal(map[string]any{
		"method": "mcpServer/elicitation/request",
		"id":     7,
		"params": map[string]any{"serverName": "recur"},
	})
	_, _ = cw.Write(append(req, '\n'))

	select {
	case res := <-answered:
		if res["action"] != "accept" {
			t.Fatalf("action=%v", res["action"])
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no answer")
	}
}
