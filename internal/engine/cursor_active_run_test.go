package engine

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

	"recur/internal/cursor"
)

func activeRunConflictFrame() []byte {
	errBody, _ := json.Marshal(map[string]any{
		"error": map[string]any{
			"code":    "internal",
			"message": "Agent agent-stuck already has active run",
		},
	})
	return cursor.EncodeConnectFrame(true, errBody)
}

func finishedRunFrame(runID, text string) []byte {
	payload := `{"result":{"agentId":"agent-stuck","runId":"` + runID + `","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"` + runID + `","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"` + text + `"}},"offset":"1"}`
	return cursor.EncodeConnectFrame(false, []byte(payload))
}

func runningRunJSON(runID, createdAt string) string {
	if createdAt == "" {
		return `{"items":[{"runId":"` + runID + `","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_RUNNING"}]}`
	}
	return `{"items":[{"runId":"` + runID + `","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_RUNNING","createdAt":"` + createdAt + `"}]}`
}

func bindStartedBridge(t *testing.T, b *cursor.Bridge, url string) time.Time {
	t.Helper()
	started := time.Now().Add(-time.Hour).UTC()
	cursor.BindClientForTest(b, url, "tok")
	cursor.SetStartedAtForTest(b, started)
	return started
}

func runStuckAgent(t *testing.T, b *cursor.Bridge) (Result, error) {
	t.Helper()
	return runStuckAgentEmit(t, b, nil)
}

func runStuckAgentEmit(t *testing.T, b *cursor.Bridge, emit func(ParsedEvent)) (Result, error) {
	t.Helper()
	r := NewCursorRunner(b, "sk-test")
	return r.Run(context.Background(), AgentSpec{
		ResumeSession: "agent-stuck",
		Worktree:      t.TempDir(),
		Model:         "composer-2.5",
		Prompt:        "continue",
	}, emit)
}

func hasParsedText(events []ParsedEvent, kind ParsedKind, text string) bool {
	for _, ev := range events {
		if ev.Kind == kind && ev.Text == text {
			return true
		}
	}
	return false
}

// TestCursorRunnerReattachesActiveRun: a same-process live run is drained via
// ObserveRun (events discarded), then this turn's prompt is Sent.
func TestCursorRunnerReattachesActiveRun(t *testing.T) {
	var sends, observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-live", createdAt))
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(finishedRunFrame("run-live", "old-run"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "delivered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	var events []ParsedEvent
	res, err := runStuckAgentEmit(t, b, func(ev ParsedEvent) {
		events = append(events, ev)
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if sends.Load() != 2 {
		t.Fatalf("Send called %d times, want 2 (conflict + this turn)", sends.Load())
	}
	if observes.Load() != 1 {
		t.Fatalf("ObserveRun called %d times, want 1", observes.Load())
	}
	if cancels.Load() != 0 {
		t.Fatalf("CancelRun called %d times, want 0 (drain, do not kill)", cancels.Load())
	}
	if res.Text != "delivered" {
		t.Fatalf("text = %q, want delivered (this turn's Send, not the drained run)", res.Text)
	}
	wait := "Waiting for previous run run-live to finish."
	fallback := "Previous run did not finish; cancelling and retrying."
	if !hasParsedText(events, KindNotice, wait) {
		t.Fatalf("events = %v, want KindNotice %q", events, wait)
	}
	if hasParsedText(events, KindText, wait) {
		t.Fatalf("events = %v, drain wait must not be KindText", events)
	}
	if hasParsedText(events, KindNotice, fallback) || hasParsedText(events, KindText, fallback) {
		t.Fatalf("events = %v, did not want cancel fallback text", events)
	}
}

// TestCursorRunnerSendAfterObserveStillConflicts: ObserveRun returning nil
// does not mean the store cleared activeRunId — a second conflict must still
// cancel and retry Send.
func TestCursorRunnerSendAfterObserveStillConflicts(t *testing.T) {
	var sends, observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-live", createdAt))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(finishedRunFrame("run-live", "old-run"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n <= 2 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-3", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	var events []ParsedEvent
	res, err := runStuckAgentEmit(t, b, func(ev ParsedEvent) {
		events = append(events, ev)
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 1 {
		t.Fatalf("ObserveRun called %d times, want 1", observes.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1 after second conflict", cancels.Load())
	}
	if sends.Load() != 3 {
		t.Fatalf("Send called %d times, want 3 (conflict + post-observe conflict + retry)", sends.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
	fallback := "Previous run did not finish; cancelling and retrying."
	if !hasParsedText(events, KindNotice, fallback) {
		t.Fatalf("events = %v, want KindNotice cancel fallback", events)
	}
	if hasParsedText(events, KindText, fallback) {
		t.Fatalf("events = %v, cancel fallback must not be KindText", events)
	}
}

// TestCursorRunnerClearsStaleActiveRun: ObserveRun cannot attach, so Recur
// cancels non-terminal runs and retries Send once.
func TestCursorRunnerClearsStaleActiveRun(t *testing.T) {
	var sends, observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-zombie", createdAt))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			errBody, _ := json.Marshal(map[string]any{
				"error": map[string]any{
					"code":    "not_found",
					"message": "Unknown run: run-zombie",
				},
			})
			_, _ = w.Write(cursor.EncodeConnectFrame(true, errBody))
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	res, err := runStuckAgent(t, b)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if sends.Load() != 2 {
		t.Fatalf("Send called %d times, want 2 (fail + retry)", sends.Load())
	}
	if observes.Load() != 1 {
		t.Fatalf("ObserveRun called %d times, want 1 (failed attach)", observes.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1", cancels.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
}

// TestCursorRunnerClearsLeftoverActiveRun: a RUNNING row created before this
// bridge process is a SQLite leftover. Recur must cancel+Send, not Observe.
func TestCursorRunnerClearsLeftoverActiveRun(t *testing.T) {
	var sends, observes, cancels atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-zombie", "2020-01-01T00:00:00Z"))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			http.NotFound(w, r)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	bindStartedBridge(t, b, srv.URL)

	res, err := runStuckAgent(t, b)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 0 {
		t.Fatalf("ObserveRun called %d times, want 0 (leftover lock)", observes.Load())
	}
	if sends.Load() != 2 {
		t.Fatalf("Send called %d times, want 2 (fail + retry)", sends.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1", cancels.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
}

// TestCursorRunnerSkipsObserveWithoutCreatedAt: missing createdAt fails closed.
func TestCursorRunnerSkipsObserveWithoutCreatedAt(t *testing.T) {
	var observes, cancels atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-unknown", ""))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			http.NotFound(w, r)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			w.Header().Set("Content-Type", "application/connect+json")
			if cancels.Load() == 0 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	bindStartedBridge(t, b, srv.URL)

	res, err := runStuckAgent(t, b)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 0 {
		t.Fatalf("ObserveRun called %d times, want 0 (unknown createdAt)", observes.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1", cancels.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
}

// TestCursorRunnerObserveIdleTimeout: replay with Offset, then only empty
// keepalives (no Offset). Idle deadline must still fire and fall back to
// cancel + Send — a total budget would also fire, but keepalives must not
// look like progress.
func TestCursorRunnerObserveIdleTimeout(t *testing.T) {
	old := observeIdleTimeout
	observeIdleTimeout = 80 * time.Millisecond
	defer func() { observeIdleTimeout = old }()

	var sends, observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-hung", createdAt))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-stuck","run_id":"run-hung","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			tick := time.NewTicker(15 * time.Millisecond)
			defer tick.Stop()
			for {
				select {
				case <-r.Context().Done():
					return
				case <-time.After(time.Second):
					return
				case <-tick.C:
					_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(`{}`)))
					if f, ok := w.(http.Flusher); ok {
						f.Flush()
					}
				}
			}
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	res, err := runStuckAgent(t, b)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 1 {
		t.Fatalf("ObserveRun called %d times, want 1", observes.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1 after idle timeout", cancels.Load())
	}
	if sends.Load() != 2 {
		t.Fatalf("Send called %d times, want 2 (conflict + retry)", sends.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
}

// TestCursorRunnerObserveIdleResetsOnOffset: Offset frames every 20ms must
// extend a 200ms idle window past what a total budget of 200ms would allow.
func TestCursorRunnerObserveIdleResetsOnOffset(t *testing.T) {
	old := observeIdleTimeout
	observeIdleTimeout = 200 * time.Millisecond
	defer func() { observeIdleTimeout = old }()

	var sends, observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-live", createdAt))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			progress := []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-stuck","run_id":"run-live","status":"RUNNING"}},"offset":"1"}`,
			)
			for i := 0; i < 15; i++ {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, progress))
				if f, ok := w.(http.Flusher); ok {
					f.Flush()
				}
				select {
				case <-r.Context().Done():
					t.Errorf("ObserveRun cancelled while still emitting offset frames")
					return
				case <-time.After(20 * time.Millisecond):
				}
			}
			_, _ = w.Write(finishedRunFrame("run-live", "old-run"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "delivered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	res, err := runStuckAgent(t, b)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 1 {
		t.Fatalf("ObserveRun called %d times, want 1", observes.Load())
	}
	if cancels.Load() != 0 {
		t.Fatalf("CancelRun called %d times, want 0 while offset frames keep arriving", cancels.Load())
	}
	if sends.Load() != 2 {
		t.Fatalf("Send called %d times, want 2 (conflict + this turn)", sends.Load())
	}
	if res.Text != "delivered" {
		t.Fatalf("text = %q, want delivered", res.Text)
	}
}

// TestCursorRunnerSkipsObserveWithoutCapability: a bridge without run.observe
// must not ObserveRun — recovery is cancel + Send.
func TestCursorRunnerSkipsObserveWithoutCapability(t *testing.T) {
	var observes, cancels atomic.Int32
	var createdAt string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/ResumeAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, runningRunJSON("run-live", createdAt))
		case strings.HasSuffix(r.URL.Path, "/ObserveRun"):
			observes.Add(1)
			http.NotFound(w, r)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			w.Header().Set("Content-Type", "application/connect+json")
			if cancels.Load() == 0 {
				_, _ = w.Write(activeRunConflictFrame())
				return
			}
			_, _ = w.Write(finishedRunFrame("run-2", "recovered"))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	started := bindStartedBridge(t, b, srv.URL)
	createdAt = started.Add(time.Minute).Format(time.RFC3339)

	r := NewCursorRunner(b, "sk-test")
	r.skipObserve = true
	res, err := r.Run(context.Background(), AgentSpec{
		ResumeSession: "agent-stuck",
		Worktree:      t.TempDir(),
		Model:         "composer-2.5",
		Prompt:        "continue",
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if observes.Load() != 0 {
		t.Fatalf("ObserveRun called %d times, want 0 (skipObserve)", observes.Load())
	}
	if cancels.Load() != 1 {
		t.Fatalf("CancelRun called %d times, want 1", cancels.Load())
	}
	if res.Text != "recovered" {
		t.Fatalf("text = %q, want recovered", res.Text)
	}
}
