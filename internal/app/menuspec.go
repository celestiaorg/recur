package app

// Menu node kinds.
const (
	MenuItem      = "item"
	MenuSeparator = "separator"
	MenuSubmenu   = "submenu"
)

// MenuNode is one entry of the application menu. The native menu (app_menu.go)
// and the in-app menu on Linux (frontend AppMenuButton) are both built from
// this tree so they cannot drift.
type MenuNode struct {
	Kind  string `json:"kind"`
	Label string `json:"label,omitempty"`
	// Action is what a click emits ("menu.action"). "quit" quits natively.
	Action string `json:"action,omitempty"`
	// EnabledKey names the MenuState flag that drives Enabled. Such items
	// start disabled until the frontend pushes "menu.state".
	EnabledKey string `json:"enabledKey,omitempty"`
	// Accel is the chord (Wails accelerator syntax). Native accelerators are
	// bound only where app_menu.go allows; the in-app menu shows it as a label.
	Accel string `json:"accel,omitempty"`
	// Dynamic marks a submenu whose children the frontend owns:
	// app-theme, editor-theme-dark, editor-theme-light, terminal-theme.
	Dynamic string `json:"dynamic,omitempty"`
	// Role is a native-only role (about, services, hide, hideOthers, unhide,
	// quit, edit, window) or "devtools", which the in-app menu also honours.
	Role     string     `json:"role,omitempty"`
	Children []MenuNode `json:"children,omitempty"`
}

func mItem(label, action, accel string) MenuNode {
	return MenuNode{Kind: MenuItem, Label: label, Action: action, Accel: accel}
}

// mCtx is a contextual item: disabled until the frontend enables enabledKey.
func mCtx(label, action, accel, enabledKey string) MenuNode {
	n := mItem(label, action, accel)
	n.EnabledKey = enabledKey
	return n
}

func mRole(role string) MenuNode { return MenuNode{Kind: MenuItem, Role: role} }

func mSep() MenuNode { return MenuNode{Kind: MenuSeparator} }

func mSub(label string, children ...MenuNode) MenuNode {
	return MenuNode{Kind: MenuSubmenu, Label: label, Children: children}
}

func mDyn(label, dynamic string) MenuNode {
	return MenuNode{Kind: MenuSubmenu, Label: label, Dynamic: dynamic}
}

// AppMenuSpec returns the application menu for goos. updates adds Check for
// Updates (macOS app menu); inspector adds Open Developer Tools.
func AppMenuSpec(goos string, updates, inspector bool) []MenuNode {
	mac := goos == "darwin"
	var spec []MenuNode

	if mac {
		appMenu := []MenuNode{mRole("about")}
		if updates {
			appMenu = append(appMenu, mItem("Check for Updates…", "check-updates", ""))
		}
		appMenu = append(appMenu,
			mSep(),
			mItem("Settings…", "settings", "CmdOrCtrl+,"),
			mSep(),
			mRole("services"),
			mSep(),
			mRole("hide"), mRole("hideOthers"), mRole("unhide"),
			mSep(),
			mRole("quit"),
		)
		spec = append(spec, mSub("Recur", appMenu...))
	}

	file := []MenuNode{
		mItem("Open…", "open-path", "CmdOrCtrl+OptionOrAlt+O"),
		mItem("New Workspace", "new-loop", "CmdOrCtrl+N"),
		// No CmdOrCtrl+Shift+N accelerator: on macOS the menubar would own that
		// chord and the webview would never see it, so Explorer's ⇧⌘N New Folder
		// (D48) would break. App.svelte owns ⇧⌘N and yields to the Files pane.
		mItem("New Blank Workspace", "new-blank-workspace", ""),
		mCtx("New Tab", "new-tab", "CmdOrCtrl+T", "newTab"),
		mCtx("Save", "save", "CmdOrCtrl+S", "save"),
		mSep(),
		mCtx("Open In…", "open-in", "CmdOrCtrl+O", "openIn"),
		mCtx("Copy Path", "copy-path", "", "copyPath"),
		mSep(),
		// Deliberately not Close Window: ⌘W is the tab-close chord.
		mCtx("Close Tab", "close-tab", "CmdOrCtrl+W", "closeTab"),
	}
	if !mac {
		// Not the Quit role: it binds CmdOrCtrl+Q natively, and on Linux Ctrl+Q
		// is the shell's XON (resume after Ctrl+S). Close the window to quit.
		file = append(file, mSep(), mItem("Quit", "quit", ""))
	}
	spec = append(spec, mSub("File", file...))

	// The Edit roles route ⌘Z/X/C/V/A through the responder chain, which
	// WKWebView needs. Off macOS WebKitGTK handles those chords itself.
	if mac {
		spec = append(spec, mRole("edit"))
	}

	view := []MenuNode{
		mSub("Appearance",
			mDyn("App Theme", "app-theme"),
			mSub("Code Editor Theme",
				mDyn("Dark", "editor-theme-dark"),
				mDyn("Light", "editor-theme-light"),
			),
			mDyn("Terminal Theme", "terminal-theme"),
			mSep(),
			mItem("Previous Editor Theme", "prev-editor-theme", "CmdOrCtrl+OptionOrAlt+["),
			mItem("Next Editor Theme", "next-editor-theme", "CmdOrCtrl+OptionOrAlt+]"),
		),
		mSep(),
		mItem("Toggle Sidebar", "toggle-sidebar", "CmdOrCtrl+B"),
		mCtx("Toggle Left Panel", "toggle-left-panel", "CmdOrCtrl+OptionOrAlt+B", "toggleLeftPanel"),
		mCtx("Toggle Right Panel", "toggle-right-panel", "CmdOrCtrl+OptionOrAlt+Shift+B", "toggleRightPanel"),
		mCtx("Toggle Editor", "toggle-editor", "CmdOrCtrl+OptionOrAlt+E", "toggleEditor"),
		mCtx("Toggle Terminal", "toggle-terminal", "Ctrl+`", "toggleTerminal"),
		mCtx("Maximize Terminal", "maximize-terminal", "Ctrl+Shift+`", "maximizeTerminal"),
		mCtx("Run…", "run-dev", "CmdOrCtrl+R", "runDev"),
		mCtx("Stop Run", "stop-run", "CmdOrCtrl+Shift+R", "stopRun"),
	}
	if !mac {
		view = append(view, mSep(), mItem("Settings…", "settings", "CmdOrCtrl+,"))
	}
	if inspector {
		view = append(view, mSep(), MenuNode{Kind: MenuItem, Label: "Open Developer Tools", Role: "devtools"})
	}
	spec = append(spec, mSub("View", view...))

	spec = append(spec, mSub("Navigate",
		mItem("Go to…", "palette", "CmdOrCtrl+K"),
		mItem("Run Command…", "run-command", "CmdOrCtrl+Shift+P"),
		mItem("Keyboard Shortcuts…", "shortcuts", "CmdOrCtrl+Shift+/"),
		mItem("Reload Bundles", "reload-bundles", ""),
		mItem("Quick Open…", "quick-open", "CmdOrCtrl+P"),
		mItem("Go to Symbol…", "go-symbol", "CmdOrCtrl+Shift+O"),
		mSep(),
		mCtx("Focus Agent Panel", "focus-agent", "CmdOrCtrl+1", "focusAgent"),
		mCtx("Focus Editor", "focus-editor", "CmdOrCtrl+2", "focusEditor"),
		mCtx("Focus Terminal", "focus-terminal", "CmdOrCtrl+3", "focusTerminal"),
		mSep(),
		mCtx("Previous Tab", "prev-tab", "CmdOrCtrl+OptionOrAlt+Left", "prevTab"),
		mCtx("Next Tab", "next-tab", "CmdOrCtrl+OptionOrAlt+Right", "nextTab"),
		mSep(),
		mItem("Previous Workspace", "prev-workspace", "CmdOrCtrl+OptionOrAlt+Up"),
		mItem("Next Workspace", "next-workspace", "CmdOrCtrl+OptionOrAlt+Down"),
	))

	// Off macOS the Window role's Minimize / Close items bind Ctrl+M (shell
	// Enter) and Ctrl+W natively; the window manager covers both.
	if mac {
		spec = append(spec, mRole("window"))
	}
	return spec
}

// HasCustomChrome reports whether this run draws its own title bar and menu
// (Linux without "Use native title bar and menu"). The saved setting may
// differ until the next launch.
func (s *ShellService) HasCustomChrome() bool { return s.CustomChrome }

// GetMenuSpec returns the application menu tree the in-app menu renders.
func (s *ShellService) GetMenuSpec() []MenuNode { return s.MenuSpec }
