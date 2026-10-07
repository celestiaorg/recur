package main

import (
	"strings"

	"github.com/wailsapp/wails/v3/pkg/application"
)

// macOS Desktop & Dock → “Double-click a window’s title bar to”.
// Observed AppleActionOnDoubleClick values: Fill, Maximize, Minimize, None.
// Unset / unknown defaults to zoom, matching AppKit.
type titlebarClickAction int

const (
	titlebarActionZoom titlebarClickAction = iota
	titlebarActionMinimize
	titlebarActionNone
)

func parseTitlebarDoubleClickAction(pref string) titlebarClickAction {
	switch strings.ToLower(strings.TrimSpace(pref)) {
	case "minimize":
		return titlebarActionMinimize
	case "none":
		return titlebarActionNone
	default:
		return titlebarActionZoom
	}
}

func titlebarDoubleClickAction() titlebarClickAction {
	return parseTitlebarDoubleClickAction(currentTitlebarDoubleClickPref())
}

// applyTitlebarDoubleClick runs the system titlebar double-click action.
// The frontend is the only trigger so it can skip buttons and traffic-light
// chrome; AppKit's own titlebar does nothing while fullscreen.
func applyTitlebarDoubleClick(win *application.WebviewWindow) {
	if win == nil || win.IsFullscreen() {
		return
	}
	switch titlebarDoubleClickAction() {
	case titlebarActionMinimize:
		win.Minimise()
	case titlebarActionNone:
		return
	default:
		// Zoom(), not Maximise(): on macOS both call -[NSWindow zoom:], but
		// Maximise() also drops min-size constraints (Wails).
		win.Zoom()
	}
}

// wireTitlebarDoubleClick listens for the frontend titlebar double-click.
func wireTitlebarDoubleClick(wapp *application.App, win *application.WebviewWindow) {
	wapp.Event.On("window.titlebarDblclick", func(*application.CustomEvent) {
		applyTitlebarDoubleClick(win)
	})
}
