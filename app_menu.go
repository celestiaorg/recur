package main

import (
	"runtime"
	"sync"

	"github.com/wailsapp/wails/v3/pkg/application"
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

	newTab           *application.MenuItem
	save             *application.MenuItem
	openIn           *application.MenuItem
	copyPath         *application.MenuItem
	closeTab         *application.MenuItem
	toggleLeftPanel  *application.MenuItem
	toggleRightPanel *application.MenuItem
	toggleEditor     *application.MenuItem
	toggleTerminal   *application.MenuItem
	maximizeTerminal *application.MenuItem
	focusAgent       *application.MenuItem
	focusEditor      *application.MenuItem
	focusTerminal    *application.MenuItem
	prevTab          *application.MenuItem
	nextTab          *application.MenuItem
	runDev           *application.MenuItem
	stopRun          *application.MenuItem

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

// installAppMenu builds the native application menu and registers keyboard
// accelerators. Menu clicks emit a "menu.action" event the frontend handles —
// on macOS the menubar owns the key equivalents, so the webview may never see
// those keydowns (see nativeMenuAccelerators for Linux). Contextual items
// start disabled; the frontend pushes "menu.state" as focus/tabs change.
func installAppMenu(wapp *application.App, updates bool) {
	emit := func(action string) func(*application.Context) {
		return func(*application.Context) {
			wapp.Event.Emit("menu.action", action)
		}
	}

	menu := wapp.NewMenu()
	items := &menuItems{
		root:             menu,
		wapp:             wapp,
		appThemeItems:    map[string]*application.MenuItem{},
		editorThemeItems: map[string]*application.MenuItem{},
	}

	// —— Recur (macOS app menu) ——
	if runtime.GOOS == "darwin" {
		appMenu := menu.AddSubmenu("Recur")
		appMenu.AddRole(application.About)
		if updates {
			appMenu.Add("Check for Updates…").OnClick(emit("check-updates"))
		}
		appMenu.AddSeparator()
		accel(appMenu.Add("Settings…"), "CmdOrCtrl+,").
			OnClick(emit("settings"))
		appMenu.AddSeparator()
		appMenu.AddRole(application.ServicesMenu)
		appMenu.AddSeparator()
		appMenu.AddRole(application.Hide)
		appMenu.AddRole(application.HideOthers)
		appMenu.AddRole(application.UnHide)
		appMenu.AddSeparator()
		appMenu.AddRole(application.Quit)
	}

	// —— File ——
	file := menu.AddSubmenu("File")
	accel(file.Add("Open…"), "CmdOrCtrl+OptionOrAlt+O").
		OnClick(emit("open-path"))
	accel(file.Add("New Workspace"), "CmdOrCtrl+N").
		OnClick(emit("new-loop"))
	// No CmdOrCtrl+Shift+N accelerator: on macOS the menubar would own that
	// chord and the webview would never see it, so Explorer's ⇧⌘N New Folder
	// (D48) would break. App.svelte owns ⇧⌘N and yields to the Files pane.
	file.Add("New Blank Workspace").
		OnClick(emit("new-blank-workspace"))
	// Contextual: new terminal tab or solo/pair agent chat when that dock is
	// focused. Disabled otherwise (editor file tabs open via Quick Open / tree).
	items.newTab = accel(file.Add("New Tab"), "CmdOrCtrl+T").
		SetEnabled(false).
		OnClick(emit("new-tab"))
	items.save = accel(file.Add("Save"), "CmdOrCtrl+S").
		SetEnabled(false).
		OnClick(emit("save"))
	file.AddSeparator()
	items.openIn = accel(file.Add("Open In…"), "CmdOrCtrl+O").
		SetEnabled(false).
		OnClick(emit("open-in"))
	items.copyPath = file.Add("Copy Path").
		SetEnabled(false).
		OnClick(emit("copy-path"))
	file.AddSeparator()
	// Contextual close: terminal / agent / file tab for the focused context.
	// Deliberately not Close Window — ⌘W is the tab-close chord.
	items.closeTab = accel(file.Add("Close Tab"), "CmdOrCtrl+W").
		SetEnabled(false).
		OnClick(emit("close-tab"))
	if runtime.GOOS != "darwin" {
		file.AddSeparator()
		// Not the Quit role: it binds CmdOrCtrl+Q natively, and on Linux Ctrl+Q
		// is the shell's XON (resume after Ctrl+S). Close the window to quit.
		file.Add("Quit").OnClick(func(*application.Context) { wapp.Quit() })
	}

	// —— Edit ——
	// macOS only: the Edit roles route ⌘Z/X/C/V/A through the responder chain,
	// which WKWebView needs. Off macOS their items are no-ops that would still
	// bind Ctrl+C/V/X/Z/A natively; WebKitGTK handles those chords itself.
	if nativeMenuAccelerators {
		menu.AddRole(application.EditMenu)
	}

	// —— View ——
	view := menu.AddSubmenu("View")
	appearance := view.AddSubmenu("Appearance")
	items.appThemeMenu = appearance.AddSubmenu("App Theme")
	editorTheme := appearance.AddSubmenu("Code Editor Theme")
	items.editorThemeDark = editorTheme.AddSubmenu("Dark")
	items.editorThemeLight = editorTheme.AddSubmenu("Light")
	items.terminalThemeMenu = appearance.AddSubmenu("Terminal Theme")
	// Placeholder until the frontend pushes the catalog after TextMate bootstrap.
	items.appThemeMenu.Add("Loading…").SetEnabled(false)
	items.editorThemeDark.Add("Loading…").SetEnabled(false)
	items.editorThemeLight.Add("Loading…").SetEnabled(false)
	items.terminalThemeMenu.Add("Loading…").SetEnabled(false)
	appearance.AddSeparator()
	accel(appearance.Add("Previous Editor Theme"), "CmdOrCtrl+OptionOrAlt+[").
		OnClick(emit("prev-editor-theme"))
	accel(appearance.Add("Next Editor Theme"), "CmdOrCtrl+OptionOrAlt+]").
		OnClick(emit("next-editor-theme"))

	view.AddSeparator()
	accel(view.Add("Toggle Sidebar"), "CmdOrCtrl+B").
		OnClick(emit("toggle-sidebar"))
	items.toggleLeftPanel = accel(view.Add("Toggle Left Panel"), "CmdOrCtrl+OptionOrAlt+B").
		SetEnabled(false).
		OnClick(emit("toggle-left-panel"))
	items.toggleRightPanel = accel(view.Add("Toggle Right Panel"), "CmdOrCtrl+OptionOrAlt+Shift+B").
		SetEnabled(false).
		OnClick(emit("toggle-right-panel"))
	items.toggleEditor = accel(view.Add("Toggle Editor"), "CmdOrCtrl+OptionOrAlt+E").
		SetEnabled(false).
		OnClick(emit("toggle-editor"))
	items.toggleTerminal = accel(view.Add("Toggle Terminal"), "Ctrl+`").
		SetEnabled(false).
		OnClick(emit("toggle-terminal"))
	items.maximizeTerminal = accel(view.Add("Maximize Terminal"), "Ctrl+Shift+`").
		SetEnabled(false).
		OnClick(emit("maximize-terminal"))
	items.runDev = accel(view.Add("Run…"), "CmdOrCtrl+R").
		SetEnabled(false).
		OnClick(emit("run-dev"))
	items.stopRun = accel(view.Add("Stop Run"), "CmdOrCtrl+Shift+R").
		SetEnabled(false).
		OnClick(emit("stop-run"))
	if runtime.GOOS != "darwin" {
		view.AddSeparator()
		accel(view.Add("Settings…"), "CmdOrCtrl+,").
			OnClick(emit("settings"))
	}
	if inspectorAvailable {
		view.AddSeparator()
		view.AddRole(application.OpenDevTools)
	}

	// —— Navigate ——
	nav := menu.AddSubmenu("Navigate")
	accel(nav.Add("Go to…"), "CmdOrCtrl+K").
		OnClick(emit("palette"))
	accel(nav.Add("Run Command…"), "CmdOrCtrl+Shift+P").
		OnClick(emit("run-command"))
	accel(nav.Add("Keyboard Shortcuts…"), "CmdOrCtrl+Shift+/").
		OnClick(emit("shortcuts"))
	nav.Add("Reload Bundles").
		OnClick(emit("reload-bundles"))
	accel(nav.Add("Quick Open…"), "CmdOrCtrl+P").
		OnClick(emit("quick-open"))
	accel(nav.Add("Go to Symbol…"), "CmdOrCtrl+Shift+O").
		OnClick(emit("go-symbol"))
	nav.AddSeparator()
	items.focusAgent = accel(nav.Add("Focus Agent Panel"), "CmdOrCtrl+1").
		SetEnabled(false).
		OnClick(emit("focus-agent"))
	items.focusEditor = accel(nav.Add("Focus Editor"), "CmdOrCtrl+2").
		SetEnabled(false).
		OnClick(emit("focus-editor"))
	items.focusTerminal = accel(nav.Add("Focus Terminal"), "CmdOrCtrl+3").
		SetEnabled(false).
		OnClick(emit("focus-terminal"))
	nav.AddSeparator()
	items.prevTab = accel(nav.Add("Previous Tab"), "CmdOrCtrl+OptionOrAlt+Left").
		SetEnabled(false).
		OnClick(emit("prev-tab"))
	items.nextTab = accel(nav.Add("Next Tab"), "CmdOrCtrl+OptionOrAlt+Right").
		SetEnabled(false).
		OnClick(emit("next-tab"))
	nav.AddSeparator()
	accel(nav.Add("Previous Workspace"), "CmdOrCtrl+OptionOrAlt+Up").
		OnClick(emit("prev-workspace"))
	accel(nav.Add("Next Workspace"), "CmdOrCtrl+OptionOrAlt+Down").
		OnClick(emit("next-workspace"))

	// —— Window ——
	// macOS only: off macOS the role's Minimize / Close items bind Ctrl+M
	// (shell Enter) and Ctrl+W natively; the window manager covers both.
	if nativeMenuAccelerators {
		menu.AddRole(application.WindowMenu)
	}

	wapp.Menu.Set(menu)

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
	set := func(item *application.MenuItem, key string) {
		if item == nil {
			return
		}
		v, ok := m[key]
		if !ok {
			return
		}
		enabled, ok := v.(bool)
		if !ok {
			return
		}
		item.SetEnabled(enabled)
	}
	set(items.newTab, "newTab")
	set(items.save, "save")
	set(items.openIn, "openIn")
	set(items.copyPath, "copyPath")
	set(items.closeTab, "closeTab")
	set(items.toggleLeftPanel, "toggleLeftPanel")
	set(items.toggleRightPanel, "toggleRightPanel")
	set(items.toggleEditor, "toggleEditor")
	set(items.toggleTerminal, "toggleTerminal")
	set(items.maximizeTerminal, "maximizeTerminal")
	set(items.focusAgent, "focusAgent")
	set(items.focusEditor, "focusEditor")
	set(items.focusTerminal, "focusTerminal")
	set(items.prevTab, "prevTab")
	set(items.nextTab, "nextTab")
	set(items.runDev, "runDev")
	set(items.stopRun, "stopRun")
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
