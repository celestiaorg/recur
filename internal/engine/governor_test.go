package engine

import (
	"testing"
	"time"
)

func TestGovernorLoopCapQueuesThird(t *testing.T) {
	g := NewGovernor(2, 8)

	if !g.TryAcquireLoop() {
		t.Fatal("first loop slot should be available")
	}
	if !g.TryAcquireLoop() {
		t.Fatal("second loop slot should be available")
	}
	// max=2 → the third must fail (the loop queues).
	if g.TryAcquireLoop() {
		t.Fatal("third loop slot must NOT be available (cap=2)")
	}
	if got := g.LoopsRunning(); got != 2 {
		t.Errorf("LoopsRunning: got %d, want 2", got)
	}

	// Releasing one frees a slot for the queued loop.
	g.ReleaseLoop()
	if !g.TryAcquireLoop() {
		t.Fatal("a slot should be free after release")
	}
}

func TestGovernorAgentCapBlocksThenReleases(t *testing.T) {
	g := NewGovernor(4, 1)
	done := make(chan struct{})

	rel1, ok := g.AcquireAgent(done)
	if !ok {
		t.Fatal("first agent slot should acquire")
	}
	if got := g.AgentsRunning(); got != 1 {
		t.Errorf("AgentsRunning: got %d, want 1", got)
	}

	// A second acquire must block; it should succeed only after the first releases.
	acquired := make(chan struct{})
	go func() {
		rel2, ok2 := g.AcquireAgent(done)
		if ok2 {
			rel2()
		}
		close(acquired)
	}()

	select {
	case <-acquired:
		t.Fatal("second agent acquire should block while cap=1 is full")
	case <-time.After(50 * time.Millisecond):
		// expected: still blocked
	}

	rel1() // free the slot
	select {
	case <-acquired:
		// good — the blocked acquire proceeded
	case <-time.After(time.Second):
		t.Fatal("second agent acquire did not proceed after release")
	}
}

func TestGovernorAgentAcquireCancels(t *testing.T) {
	g := NewGovernor(4, 1)
	done := make(chan struct{})
	rel, ok := g.AcquireAgent(done)
	if !ok {
		t.Fatal("first acquire should succeed")
	}
	defer rel()

	// Cancel before a slot frees → acquire returns ok=false promptly.
	close(done)
	_, ok2 := g.AcquireAgent(done)
	if ok2 {
		t.Fatal("acquire should fail when done is closed and no slot is free")
	}
}

func TestGovernorSetCapsRaisesLoopCap(t *testing.T) {
	g := NewGovernor(1, 8)
	if !g.TryAcquireLoop() {
		t.Fatal("first slot")
	}
	if g.TryAcquireLoop() {
		t.Fatal("cap=1 should be full")
	}
	g.SetCaps(2, 8)
	if !g.TryAcquireLoop() {
		t.Fatal("raising the loop cap must free a slot without a restart")
	}
}

func TestGovernorSetCapsWakesAgent(t *testing.T) {
	g := NewGovernor(4, 1)
	done := make(chan struct{})
	rel, ok := g.AcquireAgent(done)
	if !ok {
		t.Fatal("first agent slot")
	}
	defer rel()

	got := make(chan bool, 1)
	go func() {
		r, ok := g.AcquireAgent(done)
		if ok {
			r()
		}
		got <- ok
	}()

	select {
	case <-got:
		t.Fatal("second acquire should block at cap=1")
	case <-time.After(50 * time.Millisecond):
	}

	g.SetCaps(4, 2)
	select {
	case ok := <-got:
		if !ok {
			t.Fatal("raising the agent cap should wake the waiter")
		}
	case <-time.After(time.Second):
		t.Fatal("waiter did not proceed after SetCaps")
	}
}

func TestGovernorSetCapsLowerDoesNotPreempt(t *testing.T) {
	g := NewGovernor(2, 8)
	if first, second := g.TryAcquireLoop(), g.TryAcquireLoop(); !first || !second {
		t.Fatal("both slots should acquire")
	}
	g.SetCaps(1, 8)
	if g.TryAcquireLoop() {
		t.Fatal("lowering the cap must not free a slot while two loops are in flight")
	}
	if got := g.LoopsRunning(); got != 2 {
		t.Errorf("LoopsRunning: got %d, want 2 (in-flight work is not preempted)", got)
	}
	g.ReleaseLoop()
	if g.TryAcquireLoop() {
		t.Fatal("still at cap 1 with one loop running")
	}
	g.ReleaseLoop()
	if !g.TryAcquireLoop() {
		t.Fatal("a slot should free once in-flight work drops under the new cap")
	}
}
