package term

import (
	"encoding/base64"
	"encoding/json"
	"os/exec"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

// collector captures emitted events so tests can assert on the stream the frontend
// would have seen.
type collector struct {
	mu   sync.Mutex
	data map[string]*strings.Builder // session id -> decoded term.data
	exit map[string]int              // session id -> exit code
	idle map[string]int              // session id -> term.idle count
}

func newCollector() *collector {
	return &collector{data: map[string]*strings.Builder{}, exit: map[string]int{}, idle: map[string]int{}}
}

func (c *collector) emit(name string, payload any) {
	m, ok := payload.(map[string]any)
	if !ok {
		return
	}
	id, _ := m["id"].(string)
	c.mu.Lock()
	defer c.mu.Unlock()
	switch name {
	case "term.data":
		raw, _ := m["data"].(string)
		b, err := base64.StdEncoding.DecodeString(raw)
		if err != nil {
			return
		}
		if c.data[id] == nil {
			c.data[id] = &strings.Builder{}
		}
		c.data[id].Write(b)
	case "term.exit":
		code, _ := m["exitCode"].(int)
		c.exit[id] = code
	case "term.idle":
		c.idle[id]++
	}
}

func (c *collector) output(id string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	if b := c.data[id]; b != nil {
		return b.String()
	}
	return ""
}

func (c *collector) exited(id string) (int, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	code, ok := c.exit[id]
	return code, ok
}

func (c *collector) idleCount(id string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.idle[id]
}

// waitFor polls until cond holds or the deadline passes. PTY output is inherently
// asynchronous, so tests wait on it rather than sleeping a fixed amount.
func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func openTestSession(t *testing.T, m *Manager) Info {
	t.Helper()
	info, err := m.Open("ws-1", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = m.Close(info.ID) })
	return info
}

func TestOpenStreamsShellOutputToAttachedClient(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	if err := m.Write(info.ID, "echo recur-marker\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}

	waitFor(t, "echoed command output", func() bool {
		return strings.Contains(c.output(info.ID), "recur-marker")
	})
}

// The core detach contract: the shell keeps running while nothing is attached, its
// output lands in the replay ring, and re-attaching hands that back.
func TestDetachKeepsShellRunningAndReplaysOnReattach(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	// Let the shell reach a prompt before detaching, so the write below is read.
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	if err := m.Detach(info.ID); err != nil {
		t.Fatalf("Detach: %v", err)
	}
	before := c.output(info.ID)

	if err := m.Write(info.ID, "echo while-detached\n"); err != nil {
		t.Fatalf("Write while detached: %v", err)
	}

	var replay string
	waitFor(t, "detached output in the replay buffer", func() bool {
		snap, err := m.Attach(info.ID)
		if err != nil {
			t.Fatalf("re-Attach: %v", err)
		}
		b, err := base64.StdEncoding.DecodeString(snap)
		if err != nil {
			t.Fatalf("decode replay: %v", err)
		}
		replay = string(b)
		if strings.Contains(replay, "while-detached") {
			return true
		}
		_ = m.Detach(info.ID)
		return false
	})

	if got := c.output(info.ID); got != before {
		t.Errorf("term.data was emitted while detached:\nbefore: %q\nafter:  %q", before, got)
	}
	if !strings.Contains(replay, "while-detached") {
		t.Errorf("replay = %q, want it to contain %q", replay, "while-detached")
	}
}

// Without acks a fast writer must stall rather than queue without bound. The
// session should stop reading its PTY once unacked output passes the high-water
// mark, and resume the moment the client acks.
func TestFlowControlStallsWithoutAcksAndResumesAfter(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	// yes(1) writes far faster than any consumer, which is exactly the case flow
	// control exists for.
	if err := m.Write(info.ID, "yes recur-flood\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}

	waitFor(t, "output to pass the high-water mark", func() bool {
		return len(c.output(info.ID)) > highWater
	})

	// Once stalled, throughput must stop: no acks, no further reads.
	waitFor(t, "the stream to stall", func() bool {
		a := len(c.output(info.ID))
		time.Sleep(150 * time.Millisecond)
		return len(c.output(info.ID)) == a
	})
	stalled := len(c.output(info.ID))

	// Bytes are charged against the budget at ingest, so the reader can only
	// overshoot by the one read that crosses the mark. A looser bound here would
	// not catch the regression where the pending buffer sits outside the budget and
	// a whole flush interval's worth of output queues up behind it.
	if limit := highWater + 2*readChunk; stalled > limit {
		t.Errorf("buffered %d bytes, over the %d ceiling (high-water %d + one read); flow control is not bounding the queue",
			stalled, limit, highWater)
	}

	// Acking the whole outstanding budget must get it moving again.
	if err := m.Ack(info.ID, stalled); err != nil {
		t.Fatalf("Ack: %v", err)
	}
	waitFor(t, "the stream to resume after an ack", func() bool {
		return len(c.output(info.ID)) > stalled
	})
}

// A read loop parked in awaitCredit must not keep the session alive: detaching
// releases it so the shell isn't wedged waiting on acks that will never arrive.
func TestDetachReleasesAStalledReadLoop(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	if err := m.Write(info.ID, "yes recur-flood\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	waitFor(t, "output to pass the high-water mark", func() bool {
		return len(c.output(info.ID)) > highWater
	})

	if err := m.Detach(info.ID); err != nil {
		t.Fatalf("Detach: %v", err)
	}
	// Closing must complete promptly rather than block on the parked reader.
	done := make(chan struct{})
	go func() {
		_ = m.Close(info.ID)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("Close blocked on a stalled read loop")
	}
}

func TestExitKeepsTombstoneUntilClose(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	if err := m.Write(info.ID, "exit 3\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}

	waitFor(t, "term.exit", func() bool {
		_, ok := c.exited(info.ID)
		return ok
	})
	if code, _ := c.exited(info.ID); code != 3 {
		t.Errorf("exit code = %d, want 3", code)
	}
	got := m.List("ws-1")
	if len(got) != 1 || got[0].ID != info.ID || !got[0].Exited || got[0].ExitCode == nil || *got[0].ExitCode != 3 {
		t.Fatalf("List after exit = %+v, want tombstone exit 3", got)
	}
	m.SetActive("ws-1", info.ID)
	got = m.List("ws-1")
	if len(got) != 1 || got[0].Active {
		t.Errorf("List() Active = %v, want false for exited tombstone", got[0].Active)
	}
	if _, err := m.ReadPlain(info.ID, 20); err != nil {
		t.Fatalf("ReadPlain on tombstone: %v", err)
	}
	if err := m.Close(info.ID); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if _, err := m.get(info.ID); err == nil {
		t.Error("session still registered after Close")
	}
	if got := m.List("ws-1"); len(got) != 0 {
		t.Errorf("List = %v, want empty after Close", got)
	}
}

func TestInfoJSONKeepsCleanExitCode(t *testing.T) {
	zero := 0
	b, err := json.Marshal(Info{ID: "x", Exited: true, ExitCode: &zero})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"exitCode":0`) {
		t.Fatalf("clean exit omitted: %s", b)
	}
	live, err := json.Marshal(Info{ID: "y"})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(live), "exitCode") {
		t.Fatalf("live session leaked exitCode: %s", live)
	}
}

func TestPruneExitedTombstones(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	var ids []string
	for i := 0; i < maxExitedTombstones+2; i++ {
		info := openTestSession(t, m)
		ids = append(ids, info.ID)
		if err := m.Write(info.ID, "exit 0\n"); err != nil {
			t.Fatalf("Write: %v", err)
		}
		id := info.ID
		waitFor(t, "term.exit "+id, func() bool {
			_, ok := c.exited(id)
			return ok
		})
	}
	waitFor(t, "exited tombstones at cap", func() bool {
		n := 0
		for _, s := range m.List("ws-1") {
			if s.Exited {
				n++
			}
		}
		return n == maxExitedTombstones
	})
	live := openTestSession(t, m)

	got := m.List("ws-1")
	want := maxExitedTombstones + 1
	if len(got) != want {
		t.Fatalf("List = %d sessions, want %d (cap + live)", len(got), want)
	}
	seen := map[string]bool{}
	exited := 0
	for _, s := range got {
		seen[s.ID] = true
		if s.Exited {
			exited++
		}
	}
	if exited != maxExitedTombstones {
		t.Fatalf("exited tombstones = %d, want %d", exited, maxExitedTombstones)
	}
	if !seen[live.ID] {
		t.Fatal("live session was pruned")
	}
	if seen[ids[0]] {
		t.Fatal("oldest tombstone was not pruned")
	}
}

func TestPruneEvictsOldestExitNotOldestOpen(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	oldest := openTestSession(t, m)
	var firstDead string
	for i := 0; i < maxExitedTombstones; i++ {
		info := openTestSession(t, m)
		if i == 0 {
			firstDead = info.ID
		}
		if err := m.Write(info.ID, "exit 0\n"); err != nil {
			t.Fatalf("Write: %v", err)
		}
		id := info.ID
		waitFor(t, "term.exit "+id, func() bool {
			_, ok := c.exited(id)
			return ok
		})
	}
	if err := m.Write(oldest.ID, "exit 0\n"); err != nil {
		t.Fatalf("Write oldest: %v", err)
	}
	waitFor(t, "term.exit "+oldest.ID, func() bool {
		_, ok := c.exited(oldest.ID)
		return ok
	})
	waitFor(t, "exited tombstones at cap", func() bool {
		n := 0
		for _, s := range m.List("ws-1") {
			if s.Exited {
				n++
			}
		}
		return n == maxExitedTombstones
	})

	seen := map[string]bool{}
	for _, s := range m.List("ws-1") {
		seen[s.ID] = true
	}
	if !seen[oldest.ID] {
		t.Fatal("just-exited long-lived tab was pruned")
	}
	if seen[firstDead] {
		t.Fatal("first-exited later tab was kept")
	}
}

func TestListReturnsWorkspaceSessionsInOpenOrder(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	a := openTestSession(t, m)
	b := openTestSession(t, m)

	other, err := m.Open("ws-2", t.TempDir(), 80, 24, nil)
	if err != nil {
		t.Fatalf("Open ws-2: %v", err)
	}
	t.Cleanup(func() { _ = m.Close(other.ID) })

	got := m.List("ws-1")
	if len(got) != 2 {
		t.Fatalf("List(ws-1) returned %d sessions, want 2", len(got))
	}
	if got[0].ID != a.ID || got[1].ID != b.ID {
		t.Errorf("List order = [%s %s], want [%s %s]", got[0].ID, got[1].ID, a.ID, b.ID)
	}
	if got[0].WorkspaceID != "ws-1" {
		t.Errorf("WorkspaceID = %q, want ws-1", got[0].WorkspaceID)
	}
}

func TestReorderChangesListOrder(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	a := openTestSession(t, m)
	b := openTestSession(t, m)
	c := openTestSession(t, m)

	if err := m.Reorder("ws-1", []string{c.ID, a.ID, b.ID}); err != nil {
		t.Fatalf("Reorder(%q, [c a b]): %v", "ws-1", err)
	}
	got := m.List("ws-1")
	if len(got) != 3 {
		t.Fatalf("List(ws-1) after Reorder returned %d sessions, want 3", len(got))
	}
	gotIDs := []string{got[0].ID, got[1].ID, got[2].ID}
	want := []string{c.ID, a.ID, b.ID}
	if gotIDs[0] != want[0] || gotIDs[1] != want[1] || gotIDs[2] != want[2] {
		t.Errorf("List after Reorder = %v, want %v", gotIDs, want)
	}

	d := openTestSession(t, m)
	got = m.List("ws-1")
	if len(got) != 4 || got[3].ID != d.ID {
		last := ""
		if len(got) > 0 {
			last = got[len(got)-1].ID
		}
		t.Errorf("List after new Open last = %s (len %d), want %s (len 4)", last, len(got), d.ID)
	}

	if err := m.Close(a.ID); err != nil {
		t.Fatalf("Close(%s): %v", a.ID, err)
	}
	got = m.List("ws-1")
	gotIDs = make([]string, len(got))
	for i, info := range got {
		gotIDs[i] = info.ID
	}
	want = []string{c.ID, b.ID, d.ID}
	if len(gotIDs) != 3 || gotIDs[0] != want[0] || gotIDs[1] != want[1] || gotIDs[2] != want[2] {
		t.Errorf("List after Close = %v, want %v", gotIDs, want)
	}
}

func TestReorderRejectsNonPermutation(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	a := openTestSession(t, m)
	b := openTestSession(t, m)

	if err := m.Reorder("ws-1", []string{a.ID}); err == nil {
		t.Error("Reorder incomplete permutation: want error")
	}
	if err := m.Reorder("ws-1", []string{a.ID, b.ID, "nope"}); err == nil {
		t.Error("Reorder unknown id: want error")
	}
	if err := m.Reorder("ws-1", []string{a.ID, a.ID}); err == nil {
		t.Error("Reorder duplicate id: want error")
	}

	got := m.List("ws-1")
	if len(got) != 2 || got[0].ID != a.ID || got[1].ID != b.ID {
		t.Errorf("List after rejected Reorder = [%s %s], want [%s %s]",
			idOf(got, 0), idOf(got, 1), a.ID, b.ID)
	}
}

func idOf(got []Info, i int) string {
	if i < 0 || i >= len(got) {
		return ""
	}
	return got[i].ID
}

func TestOpenRejectsMissingWorkingDirectory(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	if _, err := m.Open("ws-1", "", 80, 24, nil); err == nil {
		t.Error("Open with empty cwd succeeded, want error")
	}
	if _, err := m.Open("ws-1", t.TempDir()+"/does-not-exist", 80, 24, nil); err == nil {
		t.Error("Open with missing cwd succeeded, want error")
	}
}

func TestOperationsOnUnknownSessionError(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	if _, err := m.Attach("nope"); err == nil {
		t.Error("Attach(nope) succeeded, want error")
	}
	for name, err := range map[string]error{
		"Detach":      m.Detach("nope"),
		"Ack":         m.Ack("nope", 1),
		"Write":       m.Write("nope", "x"),
		"Resize":      m.Resize("nope", 80, 24),
		"ClearReplay": m.ClearReplay("nope"),
		"Close":       m.Close("nope"),
	} {
		if err == nil {
			t.Errorf("%s(nope) succeeded, want error", name)
		}
	}
}

func TestClampSize(t *testing.T) {
	tests := []struct {
		name               string
		cols, rows         int
		wantCols, wantRows int
	}{
		{name: "passthrough", cols: 120, rows: 40, wantCols: 120, wantRows: 40},
		{name: "zero falls back to defaults", cols: 0, rows: 0, wantCols: 80, wantRows: 24},
		{name: "negative falls back", cols: -5, rows: -1, wantCols: 80, wantRows: 24},
		{name: "over-large is capped below uint16 wrap", cols: 99999, rows: 70000, wantCols: 1000, wantRows: 1000},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cols, rows := clampSize(tt.cols, tt.rows)
			if cols != tt.wantCols || rows != tt.wantRows {
				t.Errorf("clampSize(%d, %d) = (%d, %d), want (%d, %d)",
					tt.cols, tt.rows, cols, rows, tt.wantCols, tt.wantRows)
			}
		})
	}
}

func TestPtyEnvAdvertisesTerminalCapabilities(t *testing.T) {
	t.Setenv("TERM", "dumb")
	t.Setenv("COLUMNS", "13")
	env := ptyEnv()

	seen := map[string]int{}
	for _, kv := range env {
		if i := strings.IndexByte(kv, '='); i > 0 {
			seen[kv[:i]]++
		}
	}
	if seen["TERM"] != 1 || seen["COLORTERM"] != 1 || seen["COLORFGBG"] != 1 {
		t.Errorf("TERM/COLORTERM/COLORFGBG counts = %d/%d/%d, want exactly one of each",
			seen["TERM"], seen["COLORTERM"], seen["COLORFGBG"])
	}
	if seen["COLUMNS"] != 0 {
		t.Error("COLUMNS leaked into the PTY environment; it would fight the real window size")
	}
	for _, want := range []string{
		"TERM=xterm-256color",
		"COLORTERM=truecolor",
		"TERM_PROGRAM=Recur",
		"COLORFGBG=15;0",
	} {
		found := false
		for _, kv := range env {
			if kv == want {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("env missing %q", want)
		}
	}
}

func TestInterruptKillsForegroundKeepsShell(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	if err := m.Write(info.ID, "/bin/sleep 60\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	waitFor(t, "sleep to become the foreground group", func() bool {
		s, err := m.get(info.ID)
		if err != nil {
			return false
		}
		fg, ok := s.foregroundPgid()
		return ok && s.cmd.Process != nil && fg != s.cmd.Process.Pid
	})

	sess, err := m.get(info.ID)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	fg, _ := sess.foregroundPgid()

	if err := m.Interrupt(info.ID); err != nil {
		t.Fatalf("Interrupt: %v", err)
	}
	if !groupGone(fg) {
		t.Fatalf("process group %d still alive after Interrupt", fg)
	}

	if err := m.Write(info.ID, "echo after-interrupt\n"); err != nil {
		t.Fatalf("Write after interrupt: %v", err)
	}
	waitFor(t, "shell still accepts commands", func() bool {
		return strings.Contains(c.output(info.ID), "after-interrupt")
	})
	if _, ok := c.exited(info.ID); ok {
		t.Fatal("Interrupt tore down the session; the shell must survive")
	}
}

func TestCloseKillsJobThatIgnoresHangup(t *testing.T) {
	for name, closeAll := range map[string]bool{"Close": false, "CloseAll": true} {
		t.Run(name, func(t *testing.T) {
			c := newCollector()
			m := NewManager(c.emit)
			defer m.CloseAll()

			info := openTestSession(t, m)
			if _, err := m.Attach(info.ID); err != nil {
				t.Fatalf("Attach: %v", err)
			}
			waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

			if err := m.Write(info.ID, "/bin/sh -c \"trap '' HUP; /bin/sleep 60\"\n"); err != nil {
				t.Fatalf("Write: %v", err)
			}
			sess, err := m.get(info.ID)
			if err != nil {
				t.Fatalf("get: %v", err)
			}
			var fg int
			waitFor(t, "job to become the foreground group", func() bool {
				var ok bool
				fg, ok = sess.foregroundPgid()
				return ok && fg != sess.cmd.Process.Pid
			})

			if closeAll {
				m.CloseAll()
			} else if err := m.Close(info.ID); err != nil {
				t.Fatalf("Close: %v", err)
			}
			waitFor(t, "job group to die after close", func() bool { return groupGone(fg) })
		})
	}
}

func TestCloseSparesNohupBackgroundJob(t *testing.T) {
	// Keep the user's zsh history out of it: a shell killed mid-save leaves a
	// lock that stalls the next shell's startup.
	t.Setenv("ZDOTDIR", t.TempDir())
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	if err := m.Write(info.ID, "nohup /bin/sleep 60 >/dev/null 2>&1 & echo BG=${!}:\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	re := regexp.MustCompile(`BG=(\d+):`)
	var pid int
	waitFor(t, "background pid", func() bool {
		mm := re.FindStringSubmatch(c.output(info.ID))
		if mm == nil {
			return false
		}
		pid, _ = strconv.Atoi(mm[1])
		return true
	})
	defer func() { _ = syscall.Kill(pid, syscall.SIGKILL) }()
	waitFor(t, "nohup to exec sleep", func() bool {
		out, err := exec.Command("ps", "-o", "comm=", "-p", strconv.Itoa(pid)).Output()
		return err == nil && strings.HasSuffix(strings.TrimSpace(string(out)), "sleep")
	})

	if err := m.Close(info.ID); err != nil {
		t.Fatalf("Close: %v", err)
	}
	time.Sleep(500 * time.Millisecond)
	if err := syscall.Kill(pid, 0); err != nil {
		t.Fatalf("nohup job died with the tab: %v", err)
	}
}

func TestWatchIdleWhenCommandExitsKeepsShell(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	if err := m.Write(info.ID, "/bin/sleep 0.25\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	waitFor(t, "term.idle after sleep", func() bool { return c.idleCount(info.ID) >= 1 })
	if _, ok := c.exited(info.ID); ok {
		t.Fatalf("exited(%s) after sleep, want shell still alive", info.ID)
	}

	if err := m.Write(info.ID, "echo still-usable\n"); err != nil {
		t.Fatalf("Write after idle: %v", err)
	}
	waitFor(t, "shell still accepts commands", func() bool {
		return strings.Contains(c.output(info.ID), "still-usable")
	})
}

func TestInterruptOnIdleShellIsNoop(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	start := time.Now()
	if err := m.Interrupt(info.ID); err != nil {
		t.Fatalf("Interrupt: %v", err)
	}
	if elapsed := time.Since(start); elapsed > time.Second {
		t.Fatalf("Interrupt on an idle shell took %s", elapsed)
	}

	if err := m.Write(info.ID, "echo still-here\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	waitFor(t, "shell still accepts commands", func() bool {
		return strings.Contains(c.output(info.ID), "still-here")
	})
}

func TestInterruptEscalatesWhenSIGINTIsIgnored(t *testing.T) {
	c := newCollector()
	m := NewManager(c.emit)
	defer m.CloseAll()

	info := openTestSession(t, m)
	if _, err := m.Attach(info.ID); err != nil {
		t.Fatalf("Attach: %v", err)
	}
	waitFor(t, "initial shell output", func() bool { return c.output(info.ID) != "" })

	if err := m.Write(info.ID, "sh -c 'trap \"\" INT; echo trap-ready; exec sleep 60'\n"); err != nil {
		t.Fatalf("Write: %v", err)
	}
	waitFor(t, "trap-ready", func() bool {
		return strings.Contains(c.output(info.ID), "trap-ready")
	})
	waitFor(t, "sleep to become the foreground group", func() bool {
		s, err := m.get(info.ID)
		if err != nil {
			return false
		}
		fg, ok := s.foregroundPgid()
		return ok && s.cmd.Process != nil && fg != s.cmd.Process.Pid
	})

	sess, err := m.get(info.ID)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	fg, _ := sess.foregroundPgid()

	if err := m.Interrupt(info.ID); err != nil {
		t.Fatalf("Interrupt: %v", err)
	}
	if !groupGone(fg) {
		t.Fatalf("process group %d still alive after Interrupt", fg)
	}
	if err := m.Write(info.ID, "echo after-escalate\n"); err != nil {
		t.Fatalf("Write after interrupt: %v", err)
	}
	waitFor(t, "shell still accepts commands after SIGTERM", func() bool {
		return strings.Contains(c.output(info.ID), "after-escalate")
	})
	if _, ok := c.exited(info.ID); ok {
		t.Fatal("Interrupt tore down the session; the shell must survive")
	}
}

func TestGroupGone(t *testing.T) {
	cmd := exec.Command("/bin/sleep", "60")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		t.Fatalf("start: %v", err)
	}
	pgid := cmd.Process.Pid
	t.Cleanup(func() {
		_ = syscall.Kill(-pgid, syscall.SIGKILL)
		_ = cmd.Wait()
	})
	if groupGone(pgid) {
		t.Fatal("live process group reported gone")
	}
	if err := syscall.Kill(-pgid, syscall.SIGKILL); err != nil {
		t.Fatalf("kill: %v", err)
	}
	_ = cmd.Wait()
	waitFor(t, "process group to disappear", func() bool { return groupGone(pgid) })
}

func TestReadPlainStripsReplayAndSetTitle(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()
	info := openTestSession(t, m)
	if err := m.SetTitle(info.ID, "dev"); err != nil {
		t.Fatal(err)
	}
	got := m.List(info.WorkspaceID)
	if len(got) != 1 || got[0].Title != "dev" {
		t.Fatalf("title after SetTitle: %+v", got)
	}
	s, err := m.get(info.ID)
	if err != nil {
		t.Fatal(err)
	}
	s.mu.Lock()
	s.replay.write([]byte("ok \x1b[32mpass\x1b[0m\n"))
	s.mu.Unlock()
	plain, err := m.ReadPlain(info.ID, 20)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(plain, "pass") || strings.Contains(plain, "\x1b") {
		t.Fatalf("ReadPlain = %q", plain)
	}
	m.SetActive(info.WorkspaceID, info.ID)
	if m.ActiveID(info.WorkspaceID) != info.ID {
		t.Fatal("active not recorded")
	}
	got = m.List(info.WorkspaceID)
	if len(got) != 1 || !got[0].Active {
		t.Fatalf("List should mark the focused tab active: %+v", got)
	}
}

func TestOpenWithSeedsReplayAndTitle(t *testing.T) {
	m := NewManager(nil)
	defer m.CloseAll()

	info, err := m.OpenWith("ws-1", t.TempDir(), 80, 24, nil, OpenOptions{
		Title:  "dev",
		Replay: []byte("prior-scrollback\r\n"),
	})
	if err != nil {
		t.Fatalf("OpenWith: %v", err)
	}
	t.Cleanup(func() { _ = m.Close(info.ID) })
	if info.Title != "dev" {
		t.Errorf("title = %q, want dev", info.Title)
	}
	replay, err := m.Attach(info.ID)
	if err != nil {
		t.Fatalf("Attach: %v", err)
	}
	raw, err := base64.StdEncoding.DecodeString(replay)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(raw), "prior-scrollback") {
		t.Errorf("replay missing seeded history: %q", raw)
	}
	if !strings.Contains(string(raw), string(restoreTrailer)) {
		t.Error("replay missing restore trailer (alt-screen reset)")
	}
}

func TestRefreshCwdThrottlesFailedLookups(t *testing.T) {
	s := &Session{}
	s.refreshCwd()
	if s.cwdCheckedAt.IsZero() {
		t.Fatal("refreshCwd() cwdCheckedAt = zero after failed lookup, want stamped")
	}
	first := s.cwdCheckedAt
	s.refreshCwd()
	if !s.cwdCheckedAt.Equal(first) {
		t.Errorf("refreshCwd() cwdCheckedAt = %v, want %v (no retry inside cwdRefreshMin)", s.cwdCheckedAt, first)
	}
}
