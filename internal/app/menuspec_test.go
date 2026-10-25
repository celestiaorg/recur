package app

import "testing"

func walk(nodes []MenuNode, fn func(MenuNode)) {
	for _, n := range nodes {
		fn(n)
		walk(n.Children, fn)
	}
}

func titles(spec []MenuNode) []string {
	var out []string
	for _, n := range spec {
		out = append(out, n.Label+n.Role)
	}
	return out
}

func TestAppMenuSpecPlatforms(t *testing.T) {
	has := func(spec []MenuNode, pred func(MenuNode) bool) bool {
		found := false
		walk(spec, func(n MenuNode) { found = found || pred(n) })
		return found
	}
	quit := func(n MenuNode) bool { return n.Action == "quit" }
	role := func(r string) func(MenuNode) bool { return func(n MenuNode) bool { return n.Role == r } }
	settings := func(n MenuNode) bool { return n.Action == "settings" }

	mac := AppMenuSpec("darwin", true, false)
	linux := AppMenuSpec("linux", true, true)

	if got := titles(mac); len(got) != 6 || got[0] != "Recur" || got[2] != "edit" || got[5] != "window" {
		t.Errorf("darwin top level = %v", got)
	}
	if got := titles(linux); len(got) != 3 || got[0] != "File" || got[1] != "View" || got[2] != "Navigate" {
		t.Errorf("linux top level = %v", got)
	}
	if has(mac, quit) || !has(linux, quit) {
		t.Error("Quit item is off-macOS only")
	}
	if !has(mac, role("quit")) || has(linux, role("quit")) {
		t.Error("Quit role is macOS only")
	}
	if !has(mac, func(n MenuNode) bool { return n.Action == "check-updates" }) {
		t.Error("darwin with updates lists Check for Updates")
	}
	if has(AppMenuSpec("darwin", false, false), func(n MenuNode) bool { return n.Action == "check-updates" }) {
		t.Error("updates=false must hide Check for Updates")
	}
	if has(mac, role("devtools")) || !has(linux, role("devtools")) {
		t.Error("DevTools follows the inspector flag")
	}
	if got := countOf(mac, settings); got != 1 {
		t.Errorf("darwin Settings items = %d, want 1 (app menu)", got)
	}
	if got := countOf(linux, settings); got != 1 {
		t.Errorf("linux Settings items = %d, want 1 (View menu)", got)
	}
}

func countOf(nodes []MenuNode, pred func(MenuNode) bool) int {
	n := 0
	walk(nodes, func(m MenuNode) {
		if pred(m) {
			n++
		}
	})
	return n
}

func TestAppMenuSpecWellFormed(t *testing.T) {
	for _, goos := range []string{"darwin", "linux"} {
		walk(AppMenuSpec(goos, true, true), func(n MenuNode) {
			switch n.Kind {
			case MenuSeparator:
			case MenuSubmenu:
				if n.Label == "" || (len(n.Children) == 0) == (n.Dynamic == "") {
					t.Errorf("%s: submenu %q needs children xor dynamic", goos, n.Label)
				}
			case MenuItem:
				if n.Role == "" && (n.Label == "" || n.Action == "") {
					t.Errorf("%s: item %+v needs label and action", goos, n)
				}
			default:
				t.Errorf("%s: unknown kind %q", goos, n.Kind)
			}
		})
	}
}
