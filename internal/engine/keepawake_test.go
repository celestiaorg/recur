package engine

import (
	"runtime"
	"testing"
	"time"

	"recur/internal/store"
)

func TestAcquireAgentKeepsAwakeWhenEnabled(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("caffeinate is macOS-only")
	}
	st := store.NewInMemory()
	set := st.Settings()
	set.PreventSleepWhileAgentsRun = true
	st.SetSettings(set)

	eng := New(st)
	t.Cleanup(func() { eng.keepAwake.Close() })

	done := make(chan struct{})
	rel, ok := eng.acquireAgent(done)
	if !ok {
		t.Fatal("acquireAgent should succeed")
	}
	if !eng.keepAwake.Active() {
		t.Fatal("expected caffeinate while agent slot held and setting on")
	}
	rel()
	deadline := time.Now().Add(3 * time.Second)
	for eng.keepAwake.Active() && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	if eng.keepAwake.Active() {
		t.Fatal("expected caffeinate stopped after last agent release")
	}
}

func TestSyncKeepAwakeRespectsSettingOff(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("caffeinate is macOS-only")
	}
	st := store.NewInMemory()
	eng := New(st)
	t.Cleanup(func() { eng.keepAwake.Close() })

	done := make(chan struct{})
	rel, ok := eng.acquireAgent(done)
	if !ok {
		t.Fatal("acquireAgent should succeed")
	}
	defer rel()
	if eng.keepAwake.Active() {
		t.Fatal("default setting off — should not caffeinate")
	}

	set := st.Settings()
	set.PreventSleepWhileAgentsRun = true
	st.SetSettings(set)
	eng.SyncKeepAwake()
	if !eng.keepAwake.Active() {
		t.Fatal("expected caffeinate after enabling setting with agents running")
	}

	set.PreventSleepWhileAgentsRun = false
	st.SetSettings(set)
	eng.SyncKeepAwake()
	if eng.keepAwake.Active() {
		t.Fatal("expected caffeinate stopped after disabling setting")
	}
}

func TestKeepAwakeCloseBlocksLateSync(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("caffeinate is macOS-only")
	}
	st := store.NewInMemory()
	set := st.Settings()
	set.PreventSleepWhileAgentsRun = true
	st.SetSettings(set)

	eng := New(st)
	done := make(chan struct{})
	rel, ok := eng.acquireAgent(done)
	if !ok {
		t.Fatal("acquireAgent should succeed")
	}
	defer rel()
	if !eng.keepAwake.Active() {
		t.Fatal("expected caffeinate while agent slot held")
	}

	eng.keepAwake.Close()
	deadline := time.Now().Add(3 * time.Second)
	for eng.keepAwake.Active() && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	eng.SyncKeepAwake() // late release path during Shutdown
	if eng.keepAwake.Active() {
		t.Fatal("SyncKeepAwake after Close must not respawn caffeinate")
	}
}
