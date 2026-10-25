package main

import (
	"runtime"
	"sync"

	"github.com/wailsapp/wails/v3/pkg/application"

	"recur/internal/app"
)

// menuItems holds pointers to items whose Enabled state the frontend drives via
// the "menu.state" event (focus zone, tab counts, dirty buffer, …), plus the
// Appearance theme submenus rebuilt from "menu.themes".
//
// mu guards mutations: Wails dispatches Event.On callbacks in separate
// goroutines, and the frontend may emit menu.themes + menu.themeState (and
// menu.state) back-to-back.
type menuItems struct {
	mu   sync.Mutex
	root *application.Menu
	wapp *application.App

	// state maps a MenuNode.EnabledKey to its item.
	state map[string]*application.MenuItem

	appThemeMenu       *application.Menu
	editorThemeDark    *application.Menu
	editorThemeLight   *application.Menu
	terminalThemeMenu  *application.Menu
	appThemeItems      map[string]*application.MenuItem
	editorThemeItems   map[string]*application.MenuItem
	terminalThemeItems map[string]*application.MenuItem
}

type menuThemeEntry struct {
	Key  string `json:"key"`
	Name string `json:"name"`
	Kind string `json:"kind,omitempty"` // dark | light (editor themes)
}

type menuThemesPayload struct {
	App              []menuThemeEntry `json:"app"`
	Editor           []menuThemeEntry `json:"editor"`
	Terminal         []menuThemeEntry `json:"terminal"`
	AppSelected      string           `json:"appSelected"`
	EditorSelected   string           `json:"editorSelected"`
	TerminalSelected string           `json:"terminalSelected"`
}

// nativeMenuAccelerators reports whether menu items may carry native key
// equivalents. macOS: the menubar owns ⌘ chords and the webview may never see
// them, so the accelerator is the binding. Linux: GTK4 dispatches application
// accelerators in the window's capture phase, before WebKit, and CmdOrCtrl is
// Ctrl there, so every accelerator would steal its chord from a focused
// terminal tab (Ctrl+C interrupt, Ctrl+W delete-word, Ctrl+R history). The
// frontend handles every chord on keydown with the focused pane deciding
// first, so off macOS the menu lists the items without native bindings.
var nativeMenuAccelerators = runtime.GOOS == "darwin"

// accel attaches chord to item where native accelerators are in use.
func accel(item *application.MenuItem, chord string) *application.MenuItem {
	if nativeMenuAccelerators {
		item.SetAccelerator(chord)
	}
	return item
}

var nativeRoles = map[string]application.Role{
	"about":      application.About,
	"services":   application.ServicesMenu,
	"hide":       application.Hide,
	"hideOthers": application.HideOthers,
	"unhide":     application.UnHide,
	"quit":       application.Quit,
	"edit":       application.EditMenu,
	"window":     application.WindowMenu,
	"devtools":   application.OpenDevTools,
}

// addMenuNodes builds nodes into parent, recording enable-driven items and the
// dynamic theme submenus on items.
func (items *menuItems) addMenuNodes(parent *application.Menu, nodes []app.MenuNode, emit func(string) func(*application.Context)) {
	for _, n := range nodes {
		switch {
		case n.Role != "":
			parent.AddRole(nativeRoles[n.Role])
		case n.Kind == app.MenuSeparator:
			parent.AddSeparator()
		case n.Kind == app.MenuSubmenu:
			sub := parent.AddSubmenu(n.Label)
			switch n.Dynamic {
			case "app-theme":
				items.appThemeMenu = sub
			case "editor-theme-dark":
				items.editorThemeDark = sub
			case "editor-theme-light":
				items.editorThemeLight = sub
			case "terminal-theme":
				items.terminalThemeMenu = sub
			}
			if n.Dynamic != "" {
				// Placeholder until the frontend pushes the catalog after TextMate bootstrap.
				sub.Add("Loading…").SetEnabled(false)
			}
			items.addMenuNodes(sub, n.Children, emit)
		default:
			item := parent.Add(n.Label)
			if n.Action == "quit" {
				item.OnClick(func(*application.Context) { items.wapp.Quit() })
			} else {
				item.OnClick(emit(n.Action))
			}
			if n.EnabledKey != "" {
				item.SetEnabled(false)
				items.state[n.EnabledKey] = item
			}
			if n.Accel != "" {
				accel(item, n.Accel)
			}
		}
	}
}

// installAppMenu builds the native application menu from spec and registers
// the menu event handlers. Menu clicks emit a "menu.action" event the frontend
// handles; on macOS the menubar owns the key equivalents, so the webview may
// never see those keydowns (see nativeMenuAccelerators for Linux). Contextual
// items start disabled; the frontend pushes "menu.state" as focus/tabs change.
// With customChrome the frontend draws the menu from the same spec, so the
// native menu is built (its handlers keep working) but never attached.
func installAppMenu(wapp *application.App, spec []app.MenuNode, customChrome bool) {
	emit := func(action string) func(*application.Context) {
		return func(*application.Context) {
			wapp.Event.Emit("menu.action", action)
		}
	}

	menu := wapp.NewMenu()
	items := &menuItems{
		root:             menu,
		wapp:             wapp,
		state:            map[string]*application.MenuItem{},
		appThemeItems:    map[string]*application.MenuItem{},
		editorThemeItems: map[string]*application.MenuItem{},
	}
	items.addMenuNodes(menu, spec, emit)

	if !customChrome {
		wapp.Menu.Set(menu)
	}

	// Frontend pushes enable/disable as the workbench focus and tab counts change.
	wapp.Event.On("menu.state", func(e *application.CustomEvent) {
		applyMenuState(items, e.Data)
	})
	wapp.Event.On("menu.themes", func(e *application.CustomEvent) {
		applyMenuThemes(items, e.Data, emit)
	})
	wapp.Event.On("menu.themeState", func(e *application.CustomEvent) {
		applyMenuThemeState(items, e.Data)
	})
}

// applyMenuState toggles menubar items from a frontend-emitted map. Unknown /
// missing keys leave the current Enabled value alone so partial updates work.
func applyMenuState(items *menuItems, data any) {
	m, ok := data.(map[string]any)
	if !ok {
		return
	}
	items.mu.Lock()
	defer items.mu.Unlock()
	for key, item := range items.state {
		if enabled, ok := m[key].(bool); ok {
			item.SetEnabled(enabled)
		}
	}
	items.refresh(false)
}

// refresh rebuilds the native menubar after item changes. Linux applies
// SetEnabled/SetChecked live, and Wails' Linux Menu.Update touches GTK from the
// calling goroutine (macOS hops to the main thread itself), which segfaults
// from an event handler. So Linux rebuilds only for structural changes, and
// always on the main thread.
func (items *menuItems) refresh(structural bool) {
	if items.root == nil || (!structural && runtime.GOOS != "darwin") {
		return
	}
	application.InvokeSync(items.root.Update)
}

func decodeThemesPayload(data any) (menuThemesPayload, bool) {
	m, ok := data.(map[string]any)
	if !ok {
		return menuThemesPayload{}, false
	}
	out := menuThemesPayload{
		AppSelected:      strField(m, "appSelected"),
		EditorSelected:   strField(m, "editorSelected"),
		TerminalSelected: strField(m, "terminalSelected"),
	}
	out.App = decodeThemeEntries(m["app"])
	out.Editor = decodeThemeEntries(m["editor"])
	out.Terminal = decodeThemeEntries(m["terminal"])
	return out, true
}

func decodeThemeEntries(raw any) []menuThemeEntry {
	arr, ok := raw.([]any)
	if !ok {
		return nil
	}
	out := make([]menuThemeEntry, 0, len(arr))
	for _, item := range arr {
		m, ok := item.(map[string]any)
		if !ok {
			continue
		}
		key := strField(m, "key")
		name := strField(m, "name")
		if key == "" || name == "" {
			continue
		}
		out = append(out, menuThemeEntry{
			Key:  key,
			Name: name,
			Kind: strField(m, "kind"),
		})
	}
	return out
}

func strField(m map[string]any, key string) string {
	v, ok := m[key]
	if !ok {
		return ""
	}
	s, _ := v.(string)
	return s
}

func applyMenuThemes(items *menuItems, data any, emit func(string) func(*application.Context)) {
	payload, ok := decodeThemesPayload(data)
	if !ok || items.appThemeMenu == nil || items.terminalThemeMenu == nil {
		return
	}

	items.mu.Lock()
	defer items.mu.Unlock()

	items.appThemeMenu.Clear()
	items.editorThemeDark.Clear()
	items.editorThemeLight.Clear()
	items.terminalThemeMenu.Clear()
	items.appThemeItems = map[string]*application.MenuItem{}
	items.editorThemeItems = map[string]*application.MenuItem{}
	items.terminalThemeItems = map[string]*application.MenuItem{}

	if len(payload.App) == 0 {
		items.appThemeMenu.Add("Loading…").SetEnabled(false)
	} else {
		for _, t := range payload.App {
			checked := t.Key == payload.AppSelected
			item := items.appThemeMenu.AddRadio(t.Name, checked).
				OnClick(emit("app-theme:" + t.Key))
			items.appThemeItems[t.Key] = item
		}
	}

	var darkN, lightN int
	for _, t := range payload.Editor {
		kind := t.Kind
		if kind != "light" {
			kind = "dark"
		}
		parent := items.editorThemeDark
		if kind == "light" {
			parent = items.editorThemeLight
		}
		checked := t.Key == payload.EditorSelected
		item := parent.AddRadio(t.Name, checked).
			OnClick(emit("editor-theme:" + t.Key))
		items.editorThemeItems[t.Key] = item
		if kind == "light" {
			lightN++
		} else {
			darkN++
		}
	}
	if darkN == 0 {
		items.editorThemeDark.Add("Loading…").SetEnabled(false)
	}
	if lightN == 0 {
		items.editorThemeLight.Add("Loading…").SetEnabled(false)
	}

	if len(payload.Terminal) == 0 {
		items.terminalThemeMenu.Add("Loading…").SetEnabled(false)
	} else {
		for _, t := range payload.Terminal {
			checked := t.Key == payload.TerminalSelected
			item := items.terminalThemeMenu.AddRadio(t.Name, checked).
				OnClick(emit("terminal-theme:" + t.Key))
			items.terminalThemeItems[t.Key] = item
		}
	}

	items.refresh(true)
}

func applyMenuThemeState(items *menuItems, data any) {
	m, ok := data.(map[string]any)
	if !ok {
		return
	}
	items.mu.Lock()
	defer items.mu.Unlock()
	appSel := strField(m, "appSelected")
	edSel := strField(m, "editorSelected")
	termSel := strField(m, "terminalSelected")
	if dark, ok := m["chromeDark"].(bool); ok {
		setNativeChromeDark(dark)
	}
	for key, item := range items.appThemeItems {
		item.SetChecked(key == appSel)
	}
	for key, item := range items.editorThemeItems {
		item.SetChecked(key == edSel)
	}
	for key, item := range items.terminalThemeItems {
		item.SetChecked(key == termSel)
	}
	items.refresh(false)
}
