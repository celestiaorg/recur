package main

import (
	"testing"

	"github.com/wailsapp/wails/v3/pkg/application"
)

// accel only binds a native key equivalent where the menubar may own chords
// (macOS). Elsewhere the frontend dispatches them, so the item stays unbound.
func TestAccelFollowsNativeMenuAccelerators(t *testing.T) {
	orig := nativeMenuAccelerators
	t.Cleanup(func() { nativeMenuAccelerators = orig })

	nativeMenuAccelerators = true
	item := accel(application.NewMenuItem("Save"), "CmdOrCtrl+S")
	if got := item.GetAccelerator(); got == "" {
		t.Fatal("expected a native accelerator on macOS")
	}

	nativeMenuAccelerators = false
	item = accel(application.NewMenuItem("Save"), "CmdOrCtrl+S")
	if got := item.GetAccelerator(); got != "" {
		t.Fatalf("expected no native accelerator off macOS, got %q", got)
	}
}
