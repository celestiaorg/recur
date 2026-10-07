package cursor

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestParseConnectErrorKeepsDetails(t *testing.T) {
	raw := []byte(`{"code":"internal","message":"internal error","details":[{"type":"debug","debug":{"reason":"boom"}}]}`)
	e := parseConnectError(raw)
	if e == nil {
		t.Fatal("expected error")
	}
	if e.Code != "internal" || e.Message != "internal error" {
		t.Fatalf("code/message: %+v", e)
	}
	if !bytes.Contains(e.Details, []byte(`"reason":"boom"`)) {
		t.Fatalf("details lost: %s", e.Details)
	}
	if !strings.Contains(e.Raw, `"details"`) {
		t.Fatalf("Raw should keep the wire body, got %q", e.Raw)
	}
	// UI string stays short — details are for the log, not the toast.
	if got := e.Error(); got != "cursor bridge: [internal] internal error" {
		t.Fatalf("Error() = %q", got)
	}
}

func TestParseConnectErrorRejectsEmpty(t *testing.T) {
	if parseConnectError([]byte(`{}`)) != nil {
		t.Fatal("empty object should not parse")
	}
	if parseConnectError([]byte(`not-json`)) != nil {
		t.Fatal("garbage should not parse")
	}
}

func TestDecodeEndStreamPreservesRaw(t *testing.T) {
	frame := []byte(`{"error":{"code":"internal","message":"internal error","details":[{"x":1}]}}`)
	err := decodeEndStream(frame)
	e, ok := err.(*Error)
	if !ok || e == nil {
		t.Fatalf("got %v (%T)", err, err)
	}
	if !strings.Contains(e.Raw, `"details"`) {
		t.Fatalf("Raw missing details: %q", e.Raw)
	}
}

func TestStreamEndStreamLogsConnectBody(t *testing.T) {
	errBody := []byte(`{"error":{"code":"internal","message":"internal error","details":[{"hint":"from-cursor"}]}}`)
	frame := encodeFrame(endStreamFlag, errBody)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/connect+json")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write(frame)
	}))
	defer srv.Close()

	var logged []string
	cl := newClient(srv.URL, "tok")
	cl.log = func(line string) { logged = append(logged, line) }

	err := cl.stream(context.Background(), "sdk.v1.SdkAgentService", "Send", map[string]any{}, func(json.RawMessage) error {
		return nil
	})
	e, ok := err.(*Error)
	if !ok || e == nil || e.Code != "internal" {
		t.Fatalf("got %v (%T)", err, err)
	}
	if len(logged) != 1 {
		t.Fatalf("expected 1 log line, got %v", logged)
	}
	if !strings.Contains(logged[0], "sdk.v1.SdkAgentService/Send") {
		t.Fatalf("log should name the RPC: %s", logged[0])
	}
	if !strings.Contains(logged[0], "from-cursor") {
		t.Fatalf("log should include Connect details: %s", logged[0])
	}
}

func TestErrorUnimplemented(t *testing.T) {
	if (*Error)(nil).Unimplemented() {
		t.Error("(*Error)(nil).Unimplemented() = true, want false")
	}
	got := (&Error{Code: "unimplemented"}).Unimplemented()
	if !got {
		t.Errorf("Error{Code: unimplemented}.Unimplemented() = %v, want true", got)
	}
	got = (&Error{Code: "internal"}).Unimplemented()
	if got {
		t.Errorf("Error{Code: internal}.Unimplemented() = %v, want false", got)
	}
}

func TestUnaryConnectErrorLogsBody(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = io.WriteString(w, `{"code":"internal","message":"internal error","details":[{"hint":"unary"}]}`)
	}))
	defer srv.Close()

	var logged []string
	cl := newClient(srv.URL, "tok")
	cl.log = func(line string) { logged = append(logged, line) }

	err := cl.unary(context.Background(), "sdk.v1.SdkAgentService", "CreateAgent", map[string]any{}, nil)
	e, ok := err.(*Error)
	if !ok || e == nil {
		t.Fatalf("got %v (%T)", err, err)
	}
	if len(logged) != 1 || !strings.Contains(logged[0], "unary") {
		t.Fatalf("log = %v", logged)
	}
}

func TestSteerRunOutlivesUnaryTimeout(t *testing.T) {
	orig := unaryTimeout
	unaryTimeout = 40 * time.Millisecond
	t.Cleanup(func() { unaryTimeout = orig })

	started := make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/SteerRun") {
			http.NotFound(w, r)
			return
		}
		close(started)
		time.Sleep(120 * time.Millisecond)
		_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
	}))
	defer srv.Close()

	cl := newClient(srv.URL, "tok")
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	got, err := cl.SteerRun(ctx, "run-1", "queued behind work")
	if err != nil {
		t.Fatalf("SteerRun: %v", err)
	}
	if got != SteerCompleteDelivered {
		t.Errorf("SteerRun() = %q, want %s", got, SteerCompleteDelivered)
	}
	select {
	case <-started:
	default:
		t.Fatal("SteerRun handler never started")
	}
}
