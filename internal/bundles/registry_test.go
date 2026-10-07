package bundles_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func writeBundle(t *testing.T, root, name, toml string) string {
	t.Helper()
	dir := filepath.Join(root, name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(toml), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func TestParseManifestRejectsUnknownEnums(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		toml string
		want string
	}{
		{"input", `
[bundle]
name = "T"
id = "b"
[[command]]
id = "c1"
name = "X"
input = "documnet"
body = "#!/bin/sh\ntrue\n"
`, "unknown input"},
		{"output", `
[bundle]
name = "T"
id = "b"
[[command]]
id = "c1"
name = "X"
output = "replacedocument"
body = "#!/bin/sh\ntrue\n"
`, "unknown output"},
		{"trigger", `
[bundle]
name = "T"
id = "b"
[[command]]
id = "c1"
name = "X"
triggers = ["onSave"]
body = "#!/bin/sh\ntrue\n"
`, "unknown trigger"},
		{"showAsHTML", `
[bundle]
name = "T"
id = "b"
[[command]]
id = "c1"
name = "X"
output = "showAsHTML"
body = "#!/bin/sh\ntrue\n"
`, "showAsHTML was removed"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, _, _, err := bundles.ParseManifest(dir, []byte(tc.toml))
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("want %q in error, got %v", tc.want, err)
			}
		})
	}
}

func TestParseManifestRejectsNative(t *testing.T) {
	dir := t.TempDir()
	_, _, _, err := bundles.ParseManifest(dir, []byte(`
[bundle]
name = "T"
id = "b"
[[command]]
id = "n1"
name = "Upper"
interpreter = "native"
`))
	if err == nil || !strings.Contains(err.Error(), "not valid") {
		t.Fatalf("want reject native, got %v", err)
	}
}

func TestParseManifestRejectsJS(t *testing.T) {
	dir := t.TempDir()
	_, _, _, err := bundles.ParseManifest(dir, []byte(`
[bundle]
name = "T"
id = "b"
[[command]]
id = "j1"
name = "Upper"
interpreter = "js"
`))
	if err == nil || !strings.Contains(err.Error(), "not valid") {
		t.Fatalf("want reject js, got %v", err)
	}
}

func TestParseManifestInlineAndScript(t *testing.T) {
	dir := t.TempDir()
	bin := filepath.Join(dir, "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\necho hi\n"
	if err := os.WriteFile(filepath.Join(bin, "hi.sh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	data := []byte(`
[bundle]
name = "Demo"
id = "bundle-demo"
version = "1"

[[command]]
id = "cmd-inline"
name = "Inline"
category = "Text"
body = '''
#!/bin/sh
echo inline
'''

[[command]]
id = "cmd-script"
name = "Script"
category = "Diff"
script = "bin/hi.sh"
`)
	meta, cmds, _, err := bundles.ParseManifest(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if meta.Name != "Demo" || meta.ID != "bundle-demo" {
		t.Fatalf("meta: %+v", meta)
	}
	if len(cmds) != 2 {
		t.Fatalf("cmds: %d", len(cmds))
	}
	if !strings.Contains(cmds[0].Body, "echo inline") {
		t.Fatalf("inline body: %q", cmds[0].Body)
	}
	if cmds[1].ScriptPath == "" || !strings.Contains(cmds[1].Body, "echo hi") {
		t.Fatalf("script resolve: path=%q body=%q", cmds[1].ScriptPath, cmds[1].Body)
	}
}

func TestDiscoverUserOverridesDefaultByID(t *testing.T) {
	def := t.TempDir()
	user := t.TempDir()
	writeBundle(t, def, "Text", `
[bundle]
name = "Text"
id = "b-text"
[[command]]
id = "shared-cmd"
name = "From Default"
category = "Text"
body = "#!/bin/sh\necho default\n"
`)
	writeBundle(t, user, "Text", `
[bundle]
name = "Text"
id = "b-text-user"
[[command]]
id = "shared-cmd"
name = "From User"
category = "Text"
order = 1
body = "#!/bin/sh\necho user\n"
`)
	cmds, _, err := bundles.Discover(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	c, ok := bundles.FindByID(cmds, "shared-cmd")
	if !ok {
		t.Fatal("missing shared-cmd")
	}
	if c.Name != "From User" || c.SourceRoot != "user" {
		t.Fatalf("want user override, got %+v", c)
	}
}

func TestDiscoverSkipsMalformedBundle(t *testing.T) {
	def := t.TempDir()
	writeBundle(t, def, "Good", `
[bundle]
name = "Good"
id = "good-bundle"
[[command]]
id = "good-cmd"
name = "Ok"
body = "#!/bin/sh\ntrue\n"
`)
	writeBundle(t, def, "Bad", `
[bundle]
name = "Bad"
id = "../../../../tmp/evil"
[[command]]
id = "bad-cmd"
name = "Nope"
body = "#!/bin/sh\ntrue\n"
`)
	cmds, _, skips, err := bundles.DiscoverReport(bundles.DiscoverRoots{DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	if len(cmds) != 1 || cmds[0].ID != "good-cmd" {
		t.Fatalf("want only good-cmd, got %+v", cmds)
	}
	if len(skips) != 1 || skips[0].Name != "Bad" {
		t.Fatalf("want one skip for Bad, got %+v", skips)
	}
	if !strings.Contains(skips[0].Reason, "uuid") && !strings.Contains(skips[0].Reason, "bundle") {
		t.Fatalf("skip reason should mention schema/uuid: %q", skips[0].Reason)
	}
}

func TestDiscoverCategoryGroupingAcrossBundles(t *testing.T) {
	def := t.TempDir()
	writeBundle(t, def, "A", `
[bundle]
name = "A"
id = "b-a"
[[command]]
id = "a1"
name = "Sort"
category = "Text"
order = 2
body = "#!/bin/sh\ntrue\n"
`)
	writeBundle(t, def, "B", `
[bundle]
name = "B"
id = "b-b"
[[command]]
id = "b1"
name = "Upper"
category = "Text"
order = 1
body = "#!/bin/sh\ntrue\n"
[[command]]
id = "b2"
name = "Diff Clip"
category = "Diff"
order = 1
body = "#!/bin/sh\ntrue\n"
`)
	cmds, _, err := bundles.Discover(bundles.DiscoverRoots{DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	var text, diff int
	for _, c := range cmds {
		switch c.Category {
		case "Text":
			text++
		case "Diff":
			diff++
		}
	}
	if text != 2 || diff != 1 {
		t.Fatalf("categories text=%d diff=%d cmds=%d", text, diff, len(cmds))
	}
	// Order hint: category Text commands sorted by order then name — Upper (1) before Sort (2).
	var textNames []string
	for _, c := range cmds {
		if c.Category == "Text" {
			textNames = append(textNames, c.Name)
		}
	}
	if len(textNames) != 2 || textNames[0] != "Upper" || textNames[1] != "Sort" {
		t.Fatalf("text order: %v", textNames)
	}
}
