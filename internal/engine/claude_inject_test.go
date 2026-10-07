package engine

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// TestClaudeRunnerClosesStdinOnResult is the D49 deadlock regression: with
// `--input-format stream-json`, claude stays alive after emitting `result` until
// stdin EOF. If the runner waits for stdout to close BEFORE closing stdin, both
// sides hang forever (the stuck-coder-after-done bug). A fake "claude" that
// mirrors that contract must finish promptly under ClaudeRunner.
func TestClaudeRunnerClosesStdinOnResult(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fake claude shell script")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "fake-claude")
	// Emit a success result, then block until stdin EOF — exactly what real
	// claude does under --input-format stream-json.
	script := "#!/bin/sh\n" +
		`echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":1,"duration_ms":1,"result":"done","total_cost_usd":0.01}'` +
		"\ncat >/dev/null\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}

	inject := make(chan string, 1)
	r := &ClaudeRunner{Bin: bin}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	res, err := r.Run(ctx, AgentSpec{
		Prompt: "do the thing",
		Inject: inject,
	}, nil)
	if err != nil {
		t.Fatalf("Run: %v (likely stdin/stdout deadlock — runner must close stdin on result)", err)
	}
	if res.Subtype != "success" || res.Text != "done" {
		t.Fatalf("result: %+v", res)
	}
}

const (
	bgLive    = `echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash"}]}'` + "\n"
	bgEmpty   = `echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'` + "\n"
	bgAmbient = `echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"w1","task_type":"monitor_ws","ambient":true}]}'` + "\n"
)

func bgResult(text string) string {
	return `echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":1,"duration_ms":1,"result":"` + text + `"}'` + "\n"
}

// runFakeBgClaude runs body in a fake claude where "$EOF" exists once the
// runner has closed stdin. Returns the final result and every emitted result.
func runFakeBgClaude(t *testing.T, body string) (Result, []string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("fake claude shell script")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "fake-claude")
	script := "#!/bin/sh\nEOF='" + filepath.Join(dir, "eof") + "'\n" +
		"exec 3<&0\n(cat <&3 >/dev/null; touch \"$EOF\") &\n" + body + "wait\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	var results []string
	r := &ClaudeRunner{Bin: bin}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	res, err := r.Run(ctx, AgentSpec{Prompt: "bench", Inject: make(chan string)}, func(ev ParsedEvent) {
		if ev.Kind == KindResult {
			results = append(results, ev.Text)
		}
	})
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	return res, results
}

// A result while background tasks are live must not close stdin: claude would
// exit and kill them. The fake emits the follow-up result only if stdin is
// still open after the first one.
func TestClaudeRunnerKeepsStdinOpenForBackgroundTasks(t *testing.T) {
	res, results := runFakeBgClaude(t, bgLive+bgResult("waiting")+
		"sleep 0.3\n[ -e \"$EOF\" ] && exit 0\n"+bgEmpty+bgResult("got it"))
	if res.Text != "got it" || len(results) != 1 {
		t.Fatalf("final %q, emitted results %v; want only the post-task result", res.Text, results)
	}
}

// A resumed session first replays tasks the previous process killed as a
// model-less result. It must not close stdin before the real turn's tasks.
func TestClaudeRunnerIgnoresResumeTaskReplay(t *testing.T) {
	replay := `echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":0,"duration_ms":31,"result":"","origin":{"kind":"task-notification"}}'` + "\n"
	res, results := runFakeBgClaude(t, replay+"sleep 0.1\n"+bgLive+bgResult("waiting")+
		"sleep 0.3\n[ -e \"$EOF\" ] && exit 0\n"+bgEmpty+bgResult("got it"))
	if res.Text != "got it" || len(results) != 1 {
		t.Fatalf("final %q, emitted results %v; replay must not end the turn", res.Text, results)
	}
}

func TestClaudeRunnerIgnoresAmbientBackgroundTasks(t *testing.T) {
	res, results := runFakeBgClaude(t, bgAmbient+bgResult("done")+
		"sleep 0.3\n[ -e \"$EOF\" ] || "+bgResult("held"))
	if res.Text != "done" || len(results) != 1 {
		t.Fatalf("final %q, emitted results %v; ambient task must not hold stdin", res.Text, results)
	}
}

// Tasks ending without a follow-up turn close stdin, and the held result is
// still emitted.
func TestClaudeRunnerClosesStdinWhenBackgroundTasksEnd(t *testing.T) {
	res, results := runFakeBgClaude(t, bgLive+bgResult("waiting")+"sleep 0.1\n"+bgEmpty)
	if res.Text != "waiting" || len(results) != 1 {
		t.Fatalf("final %q, emitted results %v", res.Text, results)
	}
}

// A task that never ends (dev server) holds stdin only until the ceiling.
func TestClaudeRunnerBackgroundHoldCeiling(t *testing.T) {
	old := bgTaskHoldCeiling
	bgTaskHoldCeiling = 100 * time.Millisecond
	defer func() { bgTaskHoldCeiling = old }()
	res, results := runFakeBgClaude(t, bgLive+bgResult("server up"))
	if res.Text != "server up" || len(results) != 1 {
		t.Fatalf("final %q, emitted results %v", res.Text, results)
	}
}

func TestClaudeRunnerNacksUnflushedInjectPlain(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fake claude shell script")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "fake-claude")
	script := "#!/bin/sh\ncat >/dev/null\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}

	inject := make(chan string)
	ack := make(chan bool, 1)
	r := &ClaudeRunner{Bin: bin}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		_, err := r.Run(ctx, AgentSpec{
			Prompt:      "do the thing",
			Inject:      inject,
			InjectAck:   ack,
			InjectPlain: true,
		}, nil)
		done <- err
	}()

	deadline := time.Now().Add(3 * time.Second)
	sent := false
	for time.Now().Before(deadline) {
		select {
		case inject <- "follow-up":
			sent = true
		case err := <-done:
			t.Fatalf("Run exited before inject: %v", err)
		case <-time.After(20 * time.Millisecond):
			continue
		}
		break
	}
	if !sent {
		t.Fatal("inject was never received")
	}
	select {
	case ok := <-ack:
		t.Fatalf("acked %v before stdin write", ok)
	case <-time.After(80 * time.Millisecond):
	}
	cancel()
	select {
	case ok := <-ack:
		if ok {
			t.Fatal("unwound InjectPlain must nack")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("pending InjectPlain was not nacked on unwind")
	}
}

func TestClaudeRunnerStripsLinearAPIKey(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fake claude shell script")
	}
	t.Setenv("LINEAR_API_KEY", "secret-from-host")
	dir := t.TempDir()
	bin := filepath.Join(dir, "fake-claude")
	out := filepath.Join(dir, "env.txt")
	script := "#!/bin/sh\n" +
		"env > '" + out + "'\n" +
		`echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":1,"duration_ms":1,"result":"ok","total_cost_usd":0}'` +
		"\n"
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}

	r := &ClaudeRunner{Bin: bin}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if _, err := r.Run(ctx, AgentSpec{Prompt: "x"}, nil); err != nil {
		t.Fatalf("Run: %v", err)
	}
	if os.Getenv("LINEAR_API_KEY") != "secret-from-host" {
		t.Fatalf("host LINEAR_API_KEY = %q, want secret-from-host", os.Getenv("LINEAR_API_KEY"))
	}
	data, err := os.ReadFile(out)
	if err != nil {
		t.Fatalf("read env dump: %v", err)
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.HasPrefix(line, "LINEAR_API_KEY=") {
			t.Fatalf("claude process env still has %q", line)
		}
	}
}
