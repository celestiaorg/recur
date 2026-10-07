package engine

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"recur/internal/cursor"
)

func TestJoinSteerInjects(t *testing.T) {
	got := joinSteerInjects([]string{"  first  ", "", "second"})
	want := formatSteerUserMessage("first\n\nsecond")
	if got != want {
		t.Errorf("joinSteerInjects([first, second]) = %q, want %q", got, want)
	}
	if got := joinSteerInjects(nil); got != "" {
		t.Errorf("joinSteerInjects(nil) = %q, want empty", got)
	}
}

func TestJoinPlainInjects(t *testing.T) {
	got := joinPlainInjects([]string{"  first  ", "", "second"})
	if got != "first\n\nsecond" {
		t.Errorf("joinPlainInjects([first, second]) = %q, want %q", got, "first\n\nsecond")
	}
	if got := joinPlainInjects(nil); got != "" {
		t.Errorf("joinPlainInjects(nil) = %q, want empty", got)
	}
	plain := followUpInjectText(AgentSpec{InjectPlain: true}, []string{"stop"})
	if plain != "stop" {
		t.Errorf("followUpInjectText(plain, [stop]) = %q, want %q", plain, "stop")
	}
	enc := encodeInjectPlain("WIRE", "Fix findings")
	if followUpInjectText(AgentSpec{InjectPlain: true}, []string{enc}) != "WIRE" {
		t.Errorf("followUpInjectText(plain, encoded) = %q, want WIRE", followUpInjectText(AgentSpec{InjectPlain: true}, []string{enc}))
	}
	p, d := decodeInjectPlain(enc)
	if p != "WIRE" || d != "Fix findings" {
		t.Errorf("decodeInjectPlain(%q) = %q, %q", enc, p, d)
	}
	if got := encodeInjectPlain("same", "same"); got != "same" {
		t.Errorf("encodeInjectPlain(same, same) = %q, want same", got)
	}
	if got := injectStdinText(AgentSpec{InjectPlain: true}, enc); got != "WIRE" {
		t.Errorf("injectStdinText(plain, encoded) = %q, want WIRE", got)
	}
	if got := injectStdinText(AgentSpec{}, "stop"); got != formatSteerUserMessage("stop") {
		t.Errorf("injectStdinText(maestro, stop) = %q, want prefixed", got)
	}
	steered := followUpInjectText(AgentSpec{}, []string{"stop"})
	want := formatSteerUserMessage("stop")
	if steered != want {
		t.Errorf("followUpInjectText(maestro, [stop]) = %q, want %q", steered, want)
	}
}

func TestFireFollowUpUsersUsesDisplayText(t *testing.T) {
	var evs []ParsedEvent
	st := &cursorTurn{emit: func(ev ParsedEvent) { evs = append(evs, ev) }}
	enc := encodeInjectPlain("WIRE", "Fix findings")
	fireFollowUpUsers(st, AgentSpec{InjectPlain: true}, []string{enc}, "WIRE", true)
	if len(evs) != 1 {
		t.Fatalf("events = %d, want 1", len(evs))
	}
	if evs[0].Kind != KindUser || evs[0].Text != "WIRE" || evs[0].DisplayText != "Fix findings" {
		t.Errorf("KindUser text=%q display=%q, want WIRE / Fix findings", evs[0].Text, evs[0].DisplayText)
	}
	if !evs[0].LiveSteer {
		t.Error("LiveSteer = false, want true for delivered steer")
	}
}

func TestFireFollowUpUsersLiveFlushesText(t *testing.T) {
	var evs []ParsedEvent
	st := &cursorTurn{emit: func(ev ParsedEvent) { evs = append(evs, ev) }}
	st.thinking.WriteString("thought")
	st.text.WriteString("chunk A")
	fireFollowUpUsers(st, AgentSpec{}, []string{"steer now"}, "steer now", true)
	if st.thinking.Len() != 0 || st.text.Len() != 0 {
		t.Fatalf("buffers still hold thinking=%q text=%q", st.thinking.String(), st.text.String())
	}
	if len(evs) != 3 {
		t.Fatalf("events = %v, want thinking, text, user", kindsOf(evs))
	}
	if evs[0].Kind != KindThinking || evs[0].Text != "thought" {
		t.Errorf("events[0] = %+v, want KindThinking thought", evs[0])
	}
	if evs[1].Kind != KindText || evs[1].Text != "chunk A" {
		t.Errorf("events[1] = %+v, want KindText chunk A", evs[1])
	}
	if evs[2].Kind != KindUser || !evs[2].LiveSteer || evs[2].Text != "steer now" {
		t.Errorf("events[2] = %+v, want live KindUser steer now", evs[2])
	}
}

func TestSteerBufTakeAcksBatch(t *testing.T) {
	var s steerBuf
	s.add("first")
	batch := s.batchSeq()
	if s.consumed(batch) {
		t.Fatal("consumed before take")
	}
	if got := s.take(); len(got) != 1 || got[0] != "first" {
		t.Fatalf("take = %v, want [first]", got)
	}
	if !s.consumed(batch) {
		t.Fatal("take must ack the folded batch so CancelRun is skipped")
	}
	s.add("second")
	if s.consumed(s.batchSeq()) {
		t.Fatal("a new add must not inherit the previous ack")
	}
}

func TestSteerBufTakePrefixLeavesRemainder(t *testing.T) {
	var s steerBuf
	s.add("first")
	s.add("second")
	got := s.takePrefix(1)
	if len(got) != 1 || got[0] != "first" {
		t.Fatalf("takePrefix(1) = %v, want [first]", got)
	}
	if s.consumed(s.batchSeq()) {
		t.Fatal("remainder must not ack the batch")
	}
	rest := s.snapshot()
	if len(rest) != 1 || rest[0] != "second" {
		t.Fatalf("snapshot after takePrefix = %v, want [second]", rest)
	}
	_ = s.takePrefix(1)
	if !s.consumed(s.batchSeq()) {
		t.Fatal("empty buffer after takePrefix must ack")
	}
}

func TestSteerBufReserveExcludesTake(t *testing.T) {
	var s steerBuf
	s.add("first")
	got := s.reserve()
	if len(got) != 1 || got[0] != "first" {
		t.Fatalf("reserve() = %v, want [first]", got)
	}
	if took := s.take(); len(took) != 0 {
		t.Fatalf("take during reserve = %v, want empty", took)
	}
	if s.consumed(s.batchSeq()) {
		t.Fatal("take must not ack a reserved batch")
	}
	s.commit()
	if !s.consumed(s.batchSeq()) {
		t.Fatal("commit must ack the reserved batch")
	}

	s.add("second")
	_ = s.reserve()
	s.add("third")
	s.release()
	rest := s.take()
	if len(rest) != 2 || rest[0] != "second" || rest[1] != "third" {
		t.Fatalf("take after release = %v, want [second third]", rest)
	}
}

// TestCursorRunnerSteerCancelsThenSends is D49 on Cursor: Inject mid-run
// calls CancelRun (SDK run.cancel), concatenates every waiting steer, and
// Sends a follow-up on the same agent. Run returns success, not error_cancelled.
func TestCursorRunnerSteerCancelsThenSends(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	cancelled := make(chan struct{})
	var cancelOnce sync.Once
	var sendTexts []string
	var textsMu sync.Mutex

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-steer"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			_, _ = io.WriteString(w, `{"outcome":"revert_to_followup"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			cancelOnce.Do(func() { close(cancelled) })
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			text := sendMessageText(t, r)
			textsMu.Lock()
			sendTexts = append(sendTexts, text)
			textsMu.Unlock()
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-steer","run_id":"run-steer","status":"RUNNING"}},"offset":"1"}`,
				)))
				if f, ok := w.(http.Flusher); ok {
					f.Flush()
				}
				statusOnce.Do(func() { close(statusSent) })
				select {
				case <-cancelled:
				case <-r.Context().Done():
				}
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"result":{"agentId":"agent-steer","runId":"run-steer","status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"run-steer","agentId":"agent-steer","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}},"offset":"2"}`,
				)))
				_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
				return
			}
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-steer","runId":"run-2","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-2","agentId":"agent-steer","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"steered"}},"offset":"1"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 4)
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "first steer"
		inject <- "second steer"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if res.Text != "steered" {
		t.Errorf("Run text = %q, want steered", res.Text)
	}
	if sends.Load() != 2 {
		t.Errorf("Send called %d times, want 2 (in-flight + follow-up)", sends.Load())
	}
	if cancels.Load() < 1 {
		t.Errorf("CancelRun called %d times, want >= 1 (SDK run.cancel)", cancels.Load())
	}

	wantFollow := joinSteerInjects([]string{"first steer", "second steer"})
	textsMu.Lock()
	gotTexts := append([]string(nil), sendTexts...)
	textsMu.Unlock()
	if len(gotTexts) < 2 {
		t.Fatalf("Send texts = %v, want at least 2", gotTexts)
	}
	if gotTexts[1] != wantFollow {
		t.Errorf("Send follow-up text = %q, want %q", gotTexts[1], wantFollow)
	}

	var sawUser, sawCancelledResult bool
	for _, e := range evs {
		if e.Kind == KindUser && strings.Contains(e.Text, "first steer") && strings.Contains(e.Text, "second steer") {
			sawUser = true
		}
		if e.Kind == KindResult && e.Result != nil && e.Result.Subtype == "error_cancelled" {
			sawCancelledResult = true
		}
	}
	if !sawUser {
		t.Errorf("missing KindUser with concatenated steers, events=%v", kindsOf(evs))
	}
	if sawCancelledResult {
		t.Error("KindResult error_cancelled emitted; steer CancelRun must not end the turn")
	}
}

func TestCursorRunnerSteerCompleteDeliveredKeepsRun(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	var steers atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	steered := make(chan struct{})
	var steerOnce sync.Once
	var steerTexts []string
	var steerMu sync.Mutex

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-delivered"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			steers.Add(1)
			text := steerRunText(t, r)
			steerMu.Lock()
			steerTexts = append(steerTexts, text)
			all := strings.Join(steerTexts, "\n")
			steerMu.Unlock()
			if strings.Contains(all, "first steer") && strings.Contains(all, "second steer") {
				steerOnce.Do(func() { close(steered) })
			}
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-delivered","run_id":"run-live","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			statusOnce.Do(func() { close(statusSent) })
			select {
			case <-steered:
			case <-r.Context().Done():
			}
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-delivered","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-delivered","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"kept"}},"offset":"2"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 4)
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "first steer"
		inject <- "second steer"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if res.Text != "kept" {
		t.Errorf("Run text = %q, want kept", res.Text)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1 (delivered steer must not follow up)", sends.Load())
	}
	if cancels.Load() != 0 {
		t.Errorf("CancelRun called %d times, want 0", cancels.Load())
	}
	if steers.Load() < 1 {
		t.Errorf("SteerRun called %d times, want >= 1", steers.Load())
	}
	want := joinSteerInjects([]string{"first steer", "second steer"})
	steerMu.Lock()
	gotSteer := strings.Join(steerTexts, "\n")
	steerMu.Unlock()
	if !strings.Contains(gotSteer, "first steer") || !strings.Contains(gotSteer, "second steer") {
		t.Errorf("SteerRun texts = %q, want concatenated %q", gotSteer, want)
	}
	var sawUser bool
	for _, e := range evs {
		if e.Kind == KindUser && strings.Contains(e.Text, "first steer") {
			sawUser = true
		}
	}
	if !sawUser {
		t.Errorf("missing KindUser for delivered steer, events=%v", kindsOf(evs))
	}
}

func TestCursorRunnerCtxCancelDoesNotFollowUpSend(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stop"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			_, _ = io.WriteString(w, `{"outcome":"revert_to_followup"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-stop","run_id":"run-stop","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			// Let the client parse the status frame (noteRunID) before the
			// test cancels — otherwise Stop's CancelRun sees no run id.
			time.Sleep(50 * time.Millisecond)
			statusOnce.Do(func() { close(statusSent) })
			<-r.Context().Done()
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
		}
		cancel()
	}()

	_, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   make(chan string, 1),
	}, nil)
	if !errors.Is(err, context.Canceled) {
		t.Errorf("Run error = %v, want context.Canceled", err)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1 (Stop must not start a follow-up)", sends.Load())
	}
	if cancels.Load() < 1 {
		t.Errorf("CancelRun called %d times, want >= 1", cancels.Load())
	}
}

func TestCursorRunnerLateSteerAfterFinishedDoesNotFail(t *testing.T) {
	var sends atomic.Int32
	finished := make(chan struct{})
	var finishedOnce sync.Once

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-late"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			_, _ = io.WriteString(w, `{"outcome":"revert_to_followup"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-late","runId":"run-done","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-done","agentId":"agent-late","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"done"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			finishedOnce.Do(func() { close(finished) })
			select {
			case <-r.Context().Done():
			case <-time.After(5 * time.Second):
			}
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 1)
	go func() {
		select {
		case <-finished:
		case <-time.After(5 * time.Second):
			return
		}
		time.Sleep(50 * time.Millisecond)
		inject <- "too late"
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if res.Text != "done" {
		t.Errorf("Run text = %q, want done", res.Text)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1 (late steer must not follow up after FINISHED)", sends.Load())
	}
}

func TestCursorRunnerHandoffSteerDoesNotCancelFollowUp(t *testing.T) {
	var sends atomic.Int32
	var cancelIDs []string
	var cancelMu sync.Mutex
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	cancelled := make(chan struct{})
	var cancelOnce sync.Once
	var sendTexts []string
	var textsMu sync.Mutex

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-handoff"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			_, _ = io.WriteString(w, `{"outcome":"revert_to_followup"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			id := cancelRunID(t, r)
			cancelMu.Lock()
			cancelIDs = append(cancelIDs, id)
			cancelMu.Unlock()
			if id == "run-steer" {
				cancelOnce.Do(func() { close(cancelled) })
			}
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			text := sendMessageText(t, r)
			textsMu.Lock()
			sendTexts = append(sendTexts, text)
			textsMu.Unlock()
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-handoff","run_id":"run-steer","status":"RUNNING"}},"offset":"1"}`,
				)))
				if f, ok := w.(http.Flusher); ok {
					f.Flush()
				}
				statusOnce.Do(func() { close(statusSent) })
				select {
				case <-cancelled:
				case <-r.Context().Done():
				}
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"result":{"agentId":"agent-handoff","runId":"run-steer","status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"run-steer","agentId":"agent-handoff","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}},"offset":"2"}`,
				)))
				_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
				return
			}
			runID := "run-" + strconv.Itoa(int(n))
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-handoff","run_id":"`+runID+`","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			time.Sleep(150 * time.Millisecond)
			cancelMu.Lock()
			hitFollowUp := false
			for _, id := range cancelIDs {
				if id == runID {
					hitFollowUp = true
				}
			}
			cancelMu.Unlock()
			if hitFollowUp {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"result":{"agentId":"agent-handoff","runId":"`+runID+`","status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"`+runID+`","agentId":"agent-handoff","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}},"offset":"2"}`,
				)))
			} else {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"result":{"agentId":"agent-handoff","runId":"`+runID+`","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"`+runID+`","agentId":"agent-handoff","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"steered"}},"offset":"2"}`,
				)))
			}
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string)
	var handoffOnce sync.Once
	testBeforeFollowUpTake = func() {
		handoffOnce.Do(func() {
			inject <- "handoff steer"
			time.Sleep(50 * time.Millisecond)
		})
	}
	defer func() { testBeforeFollowUpTake = nil }()

	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "first steer"
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() < 2 {
		t.Errorf("Send called %d times, want >= 2", sends.Load())
	}
	textsMu.Lock()
	gotTexts := append([]string(nil), sendTexts...)
	textsMu.Unlock()
	var sawHandoff bool
	for _, text := range gotTexts {
		if strings.Contains(text, "handoff steer") {
			sawHandoff = true
		}
	}
	if !sawHandoff {
		t.Errorf("Send texts = %v, want handoff steer delivered on a follow-up Send", gotTexts)
	}
}

func TestCursorRunnerDeliveredSteerDuringHandoff(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	var steers atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	cancelled := make(chan struct{})
	var cancelOnce sync.Once
	var sendTexts []string
	var textsMu sync.Mutex
	var steerTexts []string
	var steerMu sync.Mutex
	var cancelIDs []string
	var cancelMu sync.Mutex

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-handoff-live"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			n := steers.Add(1)
			text := steerRunText(t, r)
			steerMu.Lock()
			steerTexts = append(steerTexts, text)
			steerMu.Unlock()
			if n == 1 {
				_, _ = io.WriteString(w, `{"outcome":"revert_to_followup"}`)
				return
			}
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			id := cancelRunID(t, r)
			cancelMu.Lock()
			cancelIDs = append(cancelIDs, id)
			cancelMu.Unlock()
			if id == "run-steer" {
				cancelOnce.Do(func() { close(cancelled) })
			}
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			n := sends.Add(1)
			text := sendMessageText(t, r)
			textsMu.Lock()
			sendTexts = append(sendTexts, text)
			textsMu.Unlock()
			w.Header().Set("Content-Type", "application/connect+json")
			if n == 1 {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-handoff-live","run_id":"run-steer","status":"RUNNING"}},"offset":"1"}`,
				)))
				if f, ok := w.(http.Flusher); ok {
					f.Flush()
				}
				statusOnce.Do(func() { close(statusSent) })
				select {
				case <-cancelled:
				case <-r.Context().Done():
				}
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"result":{"agentId":"agent-handoff-live","runId":"run-steer","status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"run-steer","agentId":"agent-handoff-live","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}},"offset":"2"}`,
				)))
				_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
				return
			}
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-handoff-live","run_id":"run-2","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			time.Sleep(150 * time.Millisecond)
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-handoff-live","runId":"run-2","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-2","agentId":"agent-handoff-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"steered"}},"offset":"2"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string)
	reserved := make(chan struct{}, 2)
	testAfterSteerReserve = func() { reserved <- struct{}{} }
	defer func() { testAfterSteerReserve = nil }()
	var handoffOnce sync.Once
	testBeforeFollowUpTake = func() {
		handoffOnce.Do(func() {
			for len(reserved) > 0 {
				<-reserved // the first steer's reserve
			}
			inject <- "handoff steer"
			// The listener's post-cancel drain can also pick it up, in which
			// case it rides the follow-up Send and is never reserved.
			select {
			case <-reserved:
			case <-time.After(2 * time.Second):
			}
		})
	}
	defer func() { testBeforeFollowUpTake = nil }()

	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "first steer"
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() != 2 {
		t.Errorf("Send called %d times, want 2", sends.Load())
	}
	cancelMu.Lock()
	gotCancels := append([]string(nil), cancelIDs...)
	cancelMu.Unlock()
	for _, id := range gotCancels {
		if id == "run-2" {
			t.Fatal("CancelRun(run-2): delivered handoff steer must keep the follow-up Send")
		}
	}
	textsMu.Lock()
	gotTexts := append([]string(nil), sendTexts...)
	textsMu.Unlock()
	steerMu.Lock()
	gotSteer := strings.Join(steerTexts, "\n")
	steerMu.Unlock()
	steered := strings.Contains(gotSteer, "handoff steer")
	folded := len(gotTexts) >= 2 && strings.Contains(gotTexts[1], "handoff steer")
	if steered == folded {
		t.Errorf("handoff steer must be delivered exactly once: SteerRun=%q Send=%v", gotSteer, gotTexts)
	}
}

func TestCursorRunnerDeliveredSteerDuringStreamComplete(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	steerStarted := make(chan struct{})
	var steerOnce sync.Once
	finishedWritten := make(chan struct{})
	var finishedOnce sync.Once

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-complete"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			steerOnce.Do(func() { close(steerStarted) })
			<-finishedWritten
			time.Sleep(50 * time.Millisecond)
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-complete","run_id":"run-live","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			<-steerStarted
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-complete","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-complete","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"done","durationMs":"1500"}},"offset":"2"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
			finishedOnce.Do(func() { close(finishedWritten) })
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 1)
	ack := make(chan bool, 1)
	go func() {
		time.Sleep(50 * time.Millisecond)
		inject <- "late-but-delivered"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree:  t.TempDir(),
		Model:     "composer-2.5",
		Prompt:    "do the thing",
		Inject:    inject,
		InjectAck: ack,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1", sends.Load())
	}
	if cancels.Load() != 0 {
		t.Errorf("CancelRun called %d times, want 0", cancels.Load())
	}
	select {
	case ok := <-ack:
		if !ok {
			t.Fatal("InjectAck = false, delivered steer must ack true")
		}
	default:
		t.Fatal("missing InjectAck for delivered steer")
	}
	liveIdx, resultIdx := -1, -1
	for i, e := range evs {
		if e.Kind == KindUser && e.LiveSteer && strings.Contains(e.Text, "late-but-delivered") {
			liveIdx = i
		}
		if e.Kind == KindResult {
			resultIdx = i
			if e.Result == nil {
				t.Fatal("KindResult carried no Result payload")
			}
			if e.Result.DurationMS != 1500 {
				t.Errorf("KindResult DurationMS = %d, want 1500", e.Result.DurationMS)
			}
		}
	}
	if liveIdx < 0 {
		t.Errorf("Run events missing live KindUser, got %v", kindsOf(evs))
	}
	if resultIdx < 0 {
		t.Errorf("Run events missing KindResult, got %v", kindsOf(evs))
	}
	if liveIdx >= 0 && resultIdx >= 0 && liveIdx > resultIdx {
		t.Errorf("event order: live KindUser index %d, KindResult index %d, want KindUser before KindResult (events=%v)", liveIdx, resultIdx, kindsOf(evs))
	}
}

func TestCursorRunnerFinishedTurnDoesNotWaitOnStuckSteerRun(t *testing.T) {
	steerStarted := make(chan struct{})
	var steerOnce sync.Once
	released := make(chan struct{})

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-stuck-steer"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			steerOnce.Do(func() { close(steerStarted) })
			select {
			case <-released:
			case <-r.Context().Done():
			}
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-stuck-steer","run_id":"run-stuck","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			<-steerStarted
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-stuck-steer","runId":"run-stuck","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-stuck","agentId":"agent-stuck-steer","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"done","durationMs":"1500"}},"offset":"2"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	defer close(released)

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 1)
	go func() {
		time.Sleep(50 * time.Millisecond)
		inject <- "steer while running"
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	started := time.Now()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, nil)
	elapsed := time.Since(started)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if elapsed > 5*time.Second {
		t.Fatalf("Run took %v, want to finish without waiting for a stuck SteerRun", elapsed)
	}
	if res.Text != "done" {
		t.Errorf("Run text = %q, want done", res.Text)
	}
}

func TestCursorRunnerSteerDuringStreamSerializesEvents(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	steered := make(chan struct{})
	var steerOnce sync.Once

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-race"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			time.Sleep(80 * time.Millisecond)
			steerOnce.Do(func() { close(steered) })
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-race","run_id":"run-live","status":"RUNNING"}},"offset":"1"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"read","status":"running","args":{"path":"a.go"}}},"offset":"2"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			statusOnce.Do(func() { close(statusSent) })
			for i := 0; i < 20; i++ {
				_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
					`{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":1000,"outputTokens":500,"cacheReadTokens":2000,"cacheWriteTokens":0,"totalTokens":3500}}},"offset":"u"}`,
				)))
				if f, ok := w.(http.Flusher); ok {
					f.Flush()
				}
				time.Sleep(5 * time.Millisecond)
			}
			select {
			case <-steered:
			case <-r.Context().Done():
			}
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"read","status":"completed","args":{"path":"a.go"},"result":{"status":"success","value":{"content":"ok"}}}},"offset":"3"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-race","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-race","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"kept"}},"offset":"4"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 1)
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "steer while streaming"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1", sends.Load())
	}
	if cancels.Load() != 0 {
		t.Errorf("CancelRun called %d times, want 0", cancels.Load())
	}
	var sawStep, sawLive, sawResult bool
	for _, e := range evs {
		if e.Kind == KindStep && e.ToolUseID == "c1" {
			sawStep = true
		}
		if e.Kind == KindUser && e.LiveSteer && strings.Contains(e.Text, "steer while streaming") {
			sawLive = true
		}
		if e.Kind == KindToolResult && e.ToolUseID == "c1" {
			sawResult = true
		}
	}
	if !sawStep || !sawLive || !sawResult {
		t.Errorf("events missing step/live-steer/result: %v", kindsOf(evs))
	}
}

func TestCursorRunnerLiveSteerFlushesStreamedText(t *testing.T) {
	var sends atomic.Int32
	var cancels atomic.Int32
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	steered := make(chan struct{})
	var steerOnce sync.Once

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/CreateAgent"):
			_, _ = io.WriteString(w, `{"agentId":"agent-text"}`)
		case strings.HasSuffix(r.URL.Path, "/CloseAgent"):
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/ListRuns"):
			_, _ = io.WriteString(w, `{"items":[]}`)
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			time.Sleep(40 * time.Millisecond)
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
			steerOnce.Do(func() { close(steered) })
		case strings.HasSuffix(r.URL.Path, "/CancelRun"):
			cancels.Add(1)
			_, _ = io.WriteString(w, `{}`)
		case strings.HasSuffix(r.URL.Path, "/Send"):
			sends.Add(1)
			w.Header().Set("Content-Type", "application/connect+json")
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-text","run_id":"run-live","status":"RUNNING"}},"offset":"1"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"assistant","message":{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"chunkA"}]}}}}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			statusOnce.Do(func() { close(statusSent) })
			select {
			case <-steered:
			case <-r.Context().Done():
			}
			time.Sleep(50 * time.Millisecond)
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"assistant","message":{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"chunkB"}]}}}}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"result":{"agentId":"agent-text","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-text","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"kept"}},"offset":"2"}`,
			)))
			_, _ = w.Write(cursor.EncodeConnectFrame(true, []byte(`{}`)))
		default:
			t.Errorf("unexpected RPC %s", r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	b := &cursor.Bridge{}
	cursor.BindClientForTest(b, srv.URL, "tok")

	inject := make(chan string, 1)
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "steer after A"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		Worktree: t.TempDir(),
		Model:    "composer-2.5",
		Prompt:   "do the thing",
		Inject:   inject,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() != 1 {
		t.Errorf("Send called %d times, want 1", sends.Load())
	}
	if cancels.Load() != 0 {
		t.Errorf("CancelRun called %d times, want 0", cancels.Load())
	}

	var textA, user, textAB, textB bool
	var sawUser bool
	for _, e := range evs {
		if e.Kind == KindUser && e.LiveSteer {
			sawUser = true
		}
		if e.Kind == KindText && e.Text == "chunkA" && !sawUser {
			textA = true
		}
		if e.Kind == KindUser && e.LiveSteer && strings.Contains(e.Text, "steer after A") {
			user = true
		}
		if e.Kind == KindText && e.Text == "chunkAchunkB" {
			textAB = true
		}
		if e.Kind == KindText && e.Text == "chunkB" && sawUser {
			textB = true
		}
	}
	if !textA || !user || !textB {
		t.Errorf("want KindText(A) then live KindUser then KindText(B), got %v", kindsOf(evs))
	}
	if textAB {
		t.Error("KindText(A+B) after live steer; pre-steer text must flush at the boundary")
	}
}

func TestCursorRunnerRecoverThenLiveSteer(t *testing.T) {
	var sends, observes, cancels, steers atomic.Int32
	var createdAt string
	var steerIDs []string
	var steerMu sync.Mutex
	statusSent := make(chan struct{})
	var statusOnce sync.Once
	steered := make(chan struct{})
	var steerOnce sync.Once

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
		case strings.HasSuffix(r.URL.Path, "/SteerRun"):
			steers.Add(1)
			id, _ := parseSteerRun(t, r)
			steerMu.Lock()
			steerIDs = append(steerIDs, id)
			steerMu.Unlock()
			steerOnce.Do(func() { close(steered) })
			_, _ = io.WriteString(w, `{"outcome":"complete_delivered"}`)
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
			_, _ = w.Write(cursor.EncodeConnectFrame(false, []byte(
				`{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-stuck","run_id":"run-2","status":"RUNNING"}},"offset":"1"}`,
			)))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			statusOnce.Do(func() { close(statusSent) })
			select {
			case <-steered:
			case <-r.Context().Done():
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

	inject := make(chan string, 1)
	go func() {
		select {
		case <-statusSent:
		case <-time.After(5 * time.Second):
			return
		}
		inject <- "after recovery"
	}()

	var evs []ParsedEvent
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := NewCursorRunner(b, "sk-test").Run(ctx, AgentSpec{
		ResumeSession: "agent-stuck",
		Worktree:      t.TempDir(),
		Model:         "composer-2.5",
		Prompt:        "continue",
		Inject:        inject,
	}, func(ev ParsedEvent) { evs = append(evs, ev) })
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if res.IsError || res.Subtype != "success" {
		t.Errorf("Run result = subtype %q isError %v, want success", res.Subtype, res.IsError)
	}
	if sends.Load() != 2 {
		t.Errorf("Send called %d times, want 2", sends.Load())
	}
	if observes.Load() != 1 {
		t.Errorf("ObserveRun called %d times, want 1", observes.Load())
	}
	if cancels.Load() != 0 {
		t.Errorf("CancelRun called %d times, want 0", cancels.Load())
	}
	if steers.Load() < 1 {
		t.Errorf("SteerRun called %d times, want >= 1", steers.Load())
	}
	steerMu.Lock()
	gotIDs := append([]string(nil), steerIDs...)
	steerMu.Unlock()
	for _, id := range gotIDs {
		if id == "run-live" {
			t.Fatalf("SteerRun(%s): recovery must steer the new Send, not the leftover run", id)
		}
	}
	if len(gotIDs) == 0 || gotIDs[0] != "run-2" {
		t.Errorf("SteerRun ids = %v, want [run-2]", gotIDs)
	}
	var sawLive bool
	for _, e := range evs {
		if e.Kind == KindUser && e.LiveSteer && strings.Contains(e.Text, "after recovery") {
			sawLive = true
		}
	}
	if !sawLive {
		t.Errorf("missing live KindUser after recovery, events=%v", kindsOf(evs))
	}
}

func sendMessageText(t *testing.T, r *http.Request) string {
	t.Helper()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		t.Errorf("read Send body: %v", err)
		return ""
	}
	payload := body
	if len(body) >= 5 {
		payload = body[5:]
	}
	var req struct {
		Message struct {
			Text string `json:"text"`
		} `json:"message"`
	}
	if err := json.Unmarshal(payload, &req); err != nil {
		t.Errorf("decode Send body: %v", err)
		return ""
	}
	return req.Message.Text
}

func steerRunText(t *testing.T, r *http.Request) string {
	t.Helper()
	_, text := parseSteerRun(t, r)
	return text
}

func parseSteerRun(t *testing.T, r *http.Request) (runID, text string) {
	t.Helper()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		t.Errorf("read SteerRun body: %v", err)
		return "", ""
	}
	var req struct {
		RunID string `json:"runId"`
		Text  string `json:"text"`
	}
	if err := json.Unmarshal(body, &req); err != nil {
		t.Errorf("decode SteerRun body: %v (%q)", err, body)
		return "", ""
	}
	return req.RunID, req.Text
}

func cancelRunID(t *testing.T, r *http.Request) string {
	t.Helper()
	body, err := io.ReadAll(r.Body)
	if err != nil {
		t.Fatalf("read CancelRun body: %v", err)
	}
	var req struct {
		RunID string `json:"runId"`
	}
	if err := json.Unmarshal(body, &req); err != nil {
		t.Fatalf("decode CancelRun body: %v (%q)", err, body)
	}
	return req.RunID
}
