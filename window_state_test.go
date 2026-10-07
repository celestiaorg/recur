package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestClampWindowState(t *testing.T) {
	st := clampWindowState(windowState{Width: 800, Height: 600})
	if st.Width != defaultWindowWidth || st.Height != defaultWindowHeight {
		t.Fatalf("undersized state should reset to defaults, got %+v", st)
	}
	st = clampWindowState(windowState{Width: 1600, Height: 1000, HasPos: true, X: 10, Y: 20})
	if st.Width != 1600 || st.Height != 1000 || !st.HasPos || st.X != 10 {
		t.Fatalf("valid state mutated: %+v", st)
	}
}

func TestLoadSaveWindowState(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dir)

	want := windowState{Width: 1500, Height: 900, X: 40, Y: 60, HasPos: true, Maximised: true}
	saveWindowState(want)

	path := filepath.Join(dir, windowStateFile)
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var file windowState
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if file != want {
		t.Fatalf("file mismatch: got %+v want %+v", file, want)
	}

	got := loadWindowState()
	if got != want {
		t.Fatalf("load mismatch: got %+v want %+v", got, want)
	}
}

func TestWindowOptionsFromState(t *testing.T) {
	opts := windowOptionsFromState(windowState{
		Width: 1500, Height: 900, X: 12, Y: 34, HasPos: true, Maximised: true,
	})
	if opts.Width != 1500 || opts.Height != 900 {
		t.Fatalf("size not applied: %dx%d", opts.Width, opts.Height)
	}
	if opts.X != 12 || opts.Y != 34 {
		t.Fatalf("position not applied: %d,%d", opts.X, opts.Y)
	}
	// Maximised is restored via Zoom() after WindowShow, not StartState
	// (Wails' create-time zoom is undone by setPosition on macOS).
	if int(opts.StartState) != 0 {
		t.Fatalf("StartState=%d, want normal (0)", opts.StartState)
	}
	if !opts.EnableFileDrop {
		t.Fatal("EnableFileDrop should be on so Finder drops reach the composer")
	}
	if opts.DevToolsEnabled != inspectorAvailable {
		t.Fatalf("DevToolsEnabled=%v, want inspectorAvailable=%v", opts.DevToolsEnabled, inspectorAvailable)
	}

	centered := windowOptionsFromState(defaultWindowState())
	if centered.InitialPosition != 0 { // WindowCentered
		t.Fatalf("default should be centered, got InitialPosition=%d", centered.InitialPosition)
	}
}

func TestWindowOptionsTitleFromInstanceLabel(t *testing.T) {
	t.Setenv("RECUR_INSTANCE_LABEL", "")
	if got := windowOptionsFromState(defaultWindowState()).Title; got != "Recur" {
		t.Errorf("windowOptionsFromState() Title = %q, want %q", got, "Recur")
	}

	t.Setenv("RECUR_INSTANCE_LABEL", "  \t")
	if got := windowOptionsFromState(defaultWindowState()).Title; got != "Recur" {
		t.Errorf("windowOptionsFromState() Title = %q, want %q", got, "Recur")
	}

	t.Setenv("RECUR_INSTANCE_LABEL", "mycodecrafting/smoky-shortcake")
	if got := windowOptionsFromState(defaultWindowState()).Title; got != "mycodecrafting/smoky-shortcake" {
		t.Errorf("windowOptionsFromState() Title = %q, want %q", got, "mycodecrafting/smoky-shortcake")
	}
}
