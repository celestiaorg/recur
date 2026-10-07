package cursor

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestActiveRunConflict(t *testing.T) {
	e := &Error{Code: "internal", Message: "Agent agent-1 already has active run"}
	if !e.ActiveRunConflict() {
		t.Fatal("expected ActiveRunConflict")
	}
	if !ActiveRunConflict(e) {
		t.Fatal("ActiveRunConflict(err) should unwrap *Error")
	}
	if ActiveRunConflict(&Error{Code: "internal", Message: "internal error"}) {
		t.Fatal("generic internal should not match")
	}
	if ActiveRunConflict(nil) {
		t.Fatal("nil should not match")
	}
}

func TestRunStatusTerminal(t *testing.T) {
	for _, s := range []string{RunFinished, RunError_, RunCancelled, RunExpired} {
		if !RunStatusTerminal(s) {
			t.Errorf("%s should be terminal", s)
		}
	}
	for _, s := range []string{RunCreating, RunRunning, "", "RUN_LIFECYCLE_STATUS_UNSPECIFIED"} {
		if RunStatusTerminal(s) {
			t.Errorf("%s should not be terminal", s)
		}
	}
}

func TestFirstNonTerminalRun(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/ListRuns") {
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
			return
		}
		_, _ = io.WriteString(w, `{"items":[
			{"runId":"run-old","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED"},
			{"runId":"run-stuck","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_RUNNING"}
		]}`)
	}))
	defer srv.Close()

	cl := newClient(srv.URL, "tok")
	run, err := cl.FirstNonTerminalRun(context.Background(), "agent-1")
	if err != nil {
		t.Fatalf("FirstNonTerminalRun: %v", err)
	}
	if run.RunID != "run-stuck" {
		t.Fatalf("run id = %q, want run-stuck", run.RunID)
	}
}

func TestObserveRun(t *testing.T) {
	var gotRunID string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/ObserveRun") {
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
			return
		}
		body, _ := io.ReadAll(r.Body)
		// Connect streaming body is a 5-byte-prefixed JSON frame.
		if len(body) > 5 {
			var req struct {
				RunID string `json:"runId"`
			}
			_ = json.Unmarshal(body[5:], &req)
			gotRunID = req.RunID
		}
		w.Header().Set("Content-Type", "application/connect+json")
		_, _ = w.Write(EncodeConnectFrame(false, []byte(
			`{"result":{"agentId":"agent-1","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok"}},"offset":"1"}`,
		)))
		_, _ = w.Write(EncodeConnectFrame(true, []byte(`{}`)))
	}))
	defer srv.Close()

	cl := newClient(srv.URL, "tok")
	var got string
	err := cl.ObserveRun(context.Background(), "run-live", "", func(ev RunStreamEvent) error {
		if ev.Result != nil && ev.Result.Result != nil {
			got = ev.Result.Result.Result
		}
		return nil
	})
	if err != nil {
		t.Fatalf("ObserveRun: %v", err)
	}
	if gotRunID != "run-live" {
		t.Fatalf("request runId = %q, want run-live", gotRunID)
	}
	if got != "ok" {
		t.Fatalf("result text = %q, want ok", got)
	}
}

func TestCancelNonTerminalRuns(t *testing.T) {
	var cancels atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		switch {
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[
				{"runId":"run-old","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED"},
				{"runId":"run-stuck","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_RUNNING"},
				{"runId":"run-creating","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_CREATING"}
			]}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			var req struct {
				RunID string `json:"runId"`
			}
			_ = json.Unmarshal(body, &req)
			if req.RunID == "run-old" {
				t.Errorf("cancelled terminal run %q", req.RunID)
			}
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	cl := newClient(srv.URL, "tok")
	n, err := cl.CancelNonTerminalRuns(context.Background(), "agent-1")
	if err != nil {
		t.Fatalf("CancelNonTerminalRuns: %v", err)
	}
	if n != 2 {
		t.Fatalf("cancelled %d, want 2", n)
	}
	if cancels.Load() != 2 {
		t.Fatalf("CancelRun called %d times, want 2", cancels.Load())
	}
}

func TestSandboxDisabledMarshalsFalse(t *testing.T) {
	// The bridge only honors an explicit enabled:false. omitempty would drop
	// the field and re-enable per-user ~/.cursor/sandbox.json.
	b, err := json.Marshal(LocalOptions{Sandbox: SandboxDisabled})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"enabled":false`) {
		t.Fatalf("sandbox disabled missing from payload: %s", b)
	}
}

func TestProtoTimeUnmarshal(t *testing.T) {
	var snap RunSnapshot
	if err := json.Unmarshal([]byte(`{"createdAt":"2026-08-26T20:11:42.000Z"}`), &snap); err != nil {
		t.Fatal(err)
	}
	if got := snap.CreatedAt.Time().UTC().Format(time.RFC3339); got != "2026-08-26T20:11:42Z" {
		t.Fatalf("RFC3339 createdAt = %s", got)
	}
	snap = RunSnapshot{}
	if err := json.Unmarshal([]byte(`{"createdAt":{"seconds":"1756239102","nanos":0}}`), &snap); err != nil {
		t.Fatal(err)
	}
	if snap.CreatedAt.Time().IsZero() {
		t.Fatal("seconds/nanos createdAt should parse")
	}
}

func TestProtoTimeMarshalJSON(t *testing.T) {
	zero, err := json.Marshal(protoTime{})
	if err != nil {
		t.Fatalf("MarshalJSON(zero) error = %v, want nil", err)
	}
	if string(zero) != "null" {
		t.Fatalf("MarshalJSON(zero) = %s, want null", zero)
	}
	ts := time.Date(2026, 8, 26, 20, 11, 42, 0, time.UTC)
	got, err := json.Marshal(protoTime(ts))
	if err != nil {
		t.Fatalf("MarshalJSON(%v) error = %v, want nil", ts, err)
	}
	if string(got) != `"2026-08-26T20:11:42Z"` {
		t.Fatalf("MarshalJSON(%v) = %s, want RFC3339", ts, got)
	}
}

func TestRunCreatedOnThisBridge(t *testing.T) {
	started := time.Date(2026, 8, 26, 20, 0, 0, 0, time.UTC)
	live := RunSnapshot{RunID: "run-1", CreatedAt: protoTime(started.Add(time.Minute))}
	old := RunSnapshot{RunID: "run-1", CreatedAt: protoTime(started.Add(-time.Hour))}
	unknown := RunSnapshot{RunID: "run-1"}
	if !RunCreatedOnThisBridge(live, started) {
		t.Fatal("run created after bridge start should observe")
	}
	if RunCreatedOnThisBridge(old, started) {
		t.Fatal("leftover run from a previous process should not observe")
	}
	if RunCreatedOnThisBridge(unknown, started) {
		t.Fatal("missing createdAt should not observe")
	}
	if RunCreatedOnThisBridge(old, time.Time{}) {
		t.Fatal("unknown bridge start should not observe")
	}
	if RunCreatedOnThisBridge(RunSnapshot{}, started) {
		t.Fatal("empty run should not observe")
	}
}
