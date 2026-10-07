package main

import "testing"

// TestRunningInAppBundle is a regression test for a real crash found by manual
// smoke-testing (Phase 5): calling the Wails notifications service from an
// unbundled binary doesn't fail gracefully, it raises an uncaught NSException that
// SIGABRTs the whole process. runningInAppBundle() gates every native-notification
// call site; this test locks its path-matching logic in place. `go test` itself
// runs as an unbundled binary, so this also incidentally proves the false case for
// the test binary itself.
func TestRunningInAppBundle(t *testing.T) {
	if runningInAppBundle() {
		t.Error("the go test binary is not a .app bundle; runningInAppBundle() should be false")
	}
}

// TestDockBadgeCount documents the dock badge arithmetic (unseen chats +
// non-chat inbox unread). Live SetBadge/RemoveBadge are called from Go after
// wapp.Run (docs + examples/badge Go event path).
func TestDockBadgeCount(t *testing.T) {
	if got := dockBadgeCount(0, 0); got != 0 {
		t.Errorf("empty: got %d", got)
	}
	if got := dockBadgeCount(3, 2); got != 5 {
		t.Errorf("stack: got %d, want 5", got)
	}
}
