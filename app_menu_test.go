package main

import (
	"testing"

	"github.com/wailsapp/wails/v3/pkg/application"

	"recur/internal/app"
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

func nativeMenuFor(t *testing.T, goos string) (*menuItems, *application.Menu, []app.MenuNode) {
	t.Helper()
	spec := app.AppMenuSpec(goos, true, true)
	menu := application.NewMenu()
	items := &menuItems{
		root:             menu,
		state:            map[string]*application.MenuItem{},
		appThemeItems:    map[string]*application.MenuItem{},
		editorThemeItems: map[string]*application.MenuItem{},
	}
	items.addMenuNodes(menu, spec, func(string) func(*application.Context) { return func(*application.Context) {} })
	return items, menu, spec
}

func countNodes(nodes []app.MenuNode, keep func(app.MenuNode) bool) int {
	n := 0
	for _, node := range nodes {
		if keep(node) {
			n++
		}
		n += countNodes(node.Children, keep)
	}
	return n
}

// The native menu is built from the spec: every enabledKey item is wired for
// menu.state and starts disabled, and the dynamic theme submenus are captured.
// Linux only: the macOS roles need a live NSApp.
func TestNativeMenuBuiltFromSpec(t *testing.T) {
	for _, goos := range []string{"linux"} {
		items, menu, spec := nativeMenuFor(t, goos)
		want := countNodes(spec, func(n app.MenuNode) bool { return n.EnabledKey != "" })
		if want != 17 || len(items.state) != want {
			t.Errorf("%s: enabledKey items = %d, wired = %d, want 17", goos, want, len(items.state))
		}
		for key, item := range items.state {
			if item.Enabled() {
				t.Errorf("%s: %s should start disabled", goos, key)
			}
		}
		if items.appThemeMenu == nil || items.editorThemeDark == nil ||
			items.editorThemeLight == nil || items.terminalThemeMenu == nil {
			t.Errorf("%s: dynamic theme submenus not captured", goos)
		}
		if menu == nil {
			t.Fatalf("%s: no menu", goos)
		}
	}
}

func TestApplyMenuStateTogglesByKey(t *testing.T) {
	items, _, _ := nativeMenuFor(t, "linux")
	applyMenuState(items, map[string]any{"save": true, "nextTab": true, "bogus": true, "closeTab": "yes"})
	for key, want := range map[string]bool{"save": true, "nextTab": true, "closeTab": false, "newTab": false} {
		if got := items.state[key].Enabled(); got != want {
			t.Errorf("%s enabled = %v, want %v", key, got, want)
		}
	}
	applyMenuState(items, map[string]any{"save": false})
	if items.state["nextTab"].Enabled() != true || items.state["save"].Enabled() {
		t.Error("partial update must leave omitted keys alone")
	}
}
