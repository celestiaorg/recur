package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"

	"recur/internal/app"
	"recur/internal/store"
)

const (
	defaultWindowWidth  = 1380
	defaultWindowHeight = 880
	minWindowWidth      = 1100
	minWindowHeight     = 720
	// titlebarHeight matches CSS `.titlebar` and the hidden-inset drag strip
	// (DECISIONS D-window).
	titlebarHeight     = 52
	windowStateFile    = "window.json"
	windowSaveDebounce = 250 * time.Millisecond
)

// windowState is the last known main-window geometry, persisted under the
// managed data root so size/position survive restarts.
type windowState struct {
	Width     int  `json:"width"`
	Height    int  `json:"height"`
	X         int  `json:"x"`
	Y         int  `json:"y"`
	HasPos    bool `json:"hasPos"`
	Maximised bool `json:"maximised"`
}

func defaultWindowState() windowState {
	return windowState{Width: defaultWindowWidth, Height: defaultWindowHeight}
}

func windowStatePath() (string, error) {
	root, err := store.DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, windowStateFile), nil
}

func loadWindowState() windowState {
	path, err := windowStatePath()
	if err != nil {
		return defaultWindowState()
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return defaultWindowState()
	}
	var st windowState
	if err := json.Unmarshal(data, &st); err != nil {
		return defaultWindowState()
	}
	return clampWindowState(st)
}

func clampWindowState(st windowState) windowState {
	if st.Width < minWindowWidth {
		st.Width = defaultWindowWidth
	}
	if st.Height < minWindowHeight {
		st.Height = defaultWindowHeight
	}
	return st
}

func saveWindowState(st windowState) {
	st = clampWindowState(st)
	path, err := windowStatePath()
	if err != nil {
		return
	}
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		return
	}
	_ = os.WriteFile(path, data, 0o644)
}

// windowOptionsFromState builds WebviewWindowOptions with restored geometry.
func windowOptionsFromState(st windowState) application.WebviewWindowOptions {
	opts := application.WebviewWindowOptions{
		Title:              app.WindowTitle(),
		Width:              st.Width,
		Height:             st.Height,
		MinWidth:           minWindowWidth,
		MinHeight:          minWindowHeight,
		BackgroundColour:   application.NewRGB(10, 12, 16),
		UseApplicationMenu: true,
		EnableFileDrop:     true,
		// True in `wails3 task dev` and packages built with RECUR_DEBUG=1.
		DevToolsEnabled: inspectorAvailable,
		Mac: application.MacWindow{
			InvisibleTitleBarHeight: titlebarHeight,
			Backdrop:                application.MacBackdropNormal,
			TitleBar:                application.MacTitleBarHiddenInset,
		},
		URL: "/",
	}
	if st.HasPos {
		opts.InitialPosition = application.WindowXY
		opts.X = st.X
		opts.Y = st.Y
	}
	// Maximised is not mapped to StartState: on macOS Wails zooms during
	// create and then calls setPosition, which undoes the zoom. Instead
	// persistWindowGeometry calls Zoom() once the window is shown.
	return opts
}

// withCustomChrome drops the native title bar and menu bar; the frontend draws
// both (Linux, unless the user opted into the native chrome).
func withCustomChrome(opts application.WebviewWindowOptions) application.WebviewWindowOptions {
	opts.Frameless = true
	opts.UseApplicationMenu = false
	return opts
}

// persistWindowGeometry watches resize/move/close and writes window.json.
// Size is only recorded while the window is in a normal (non-maximised /
// non-fullscreen / non-minimised) state so a maximised session doesn't clobber
// the last restored dimensions.
func persistWindowGeometry(win *application.WebviewWindow) {
	var (
		mu     sync.Mutex
		timer  *time.Timer
		normal = loadWindowState()
	)

	snapshot := func() windowState {
		maximised := win.IsMaximised()
		if !win.IsMinimised() && !win.IsFullscreen() && !maximised {
			w, h := win.Size()
			x, y := win.Position()
			st := windowState{
				Width: w, Height: h,
				X: x, Y: y,
				HasPos: true,
			}
			mu.Lock()
			normal = st
			mu.Unlock()
			return st
		}
		mu.Lock()
		out := normal
		mu.Unlock()
		out.Maximised = maximised
		return out
	}

	flush := func() {
		mu.Lock()
		if timer != nil {
			timer.Stop()
			timer = nil
		}
		mu.Unlock()
		saveWindowState(snapshot())
	}

	schedule := func() {
		mu.Lock()
		defer mu.Unlock()
		if timer != nil {
			timer.Stop()
		}
		timer = time.AfterFunc(windowSaveDebounce, func() {
			saveWindowState(snapshot())
		})
	}

	if normal.Maximised {
		var once sync.Once
		win.OnWindowEvent(events.Common.WindowShow, func(*application.WindowEvent) {
			once.Do(func() {
				if !win.IsMaximised() {
					win.Zoom()
				}
			})
		})
	}

	win.OnWindowEvent(events.Common.WindowDidResize, func(*application.WindowEvent) { schedule() })
	win.OnWindowEvent(events.Common.WindowDidMove, func(*application.WindowEvent) { schedule() })
	win.OnWindowEvent(events.Common.WindowClosing, func(*application.WindowEvent) { flush() })
}
