package bundles_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"

	"recur/internal/bundles"
)

func TestParseSettings(t *testing.T) {
	dir := t.TempDir()
	data := []byte(`
[bundle]
name = "Linear"
id = "recur.linear"

[[setting]]
id = "recur.linear.api-key"
name = "api_key"
label = "API key"
secret = true
placeholder = "lin_api_…"
url = "https://linear.app/settings/api"
urlLabel = "Open Linear"

[[setting]]
id = "recur.linear.team-key"
name = "team_key"
label = "Default team key"
placeholder = "ENG"
`)
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Settings) != 2 {
		t.Fatalf("settings=%d", len(parsed.Settings))
	}
	s := parsed.Settings[0]
	if s.ID != "recur.linear.api-key" || s.Name != "api_key" || !s.Secret || s.URLLabel != "Open Linear" {
		t.Fatalf("api key setting=%+v", s)
	}
	if bundles.SettingEnvName(s.Name) != "RECUR_SETTING_API_KEY" {
		t.Fatalf("env name=%q", bundles.SettingEnvName(s.Name))
	}
}

func TestParseSettingsRejects(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		toml string
		want string
	}{
		{"bad name", `
[bundle]
id = "b"
name = "B"
[[setting]]
id = "b.k"
name = "API-KEY"
`, "name"},
		{"dup name", `
[bundle]
id = "b"
name = "B"
[[setting]]
id = "b.one"
name = "token"
[[setting]]
id = "b.two"
name = "token"
`, "duplicate name"},
		{"dup id", `
[bundle]
id = "b"
name = "B"
[[setting]]
id = "b.one"
name = "a"
[[setting]]
id = "b.one"
name = "b"
`, "duplicate id"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := bundles.ParseManifestFull(dir, []byte(tc.toml))
			if err == nil || !strings.Contains(strings.ToLower(err.Error()), tc.want) {
				t.Fatalf("err=%v want %q", err, tc.want)
			}
		})
	}
}

func TestParseSettingsCap(t *testing.T) {
	dir := t.TempDir()
	var b strings.Builder
	b.WriteString("[bundle]\nid = \"b\"\nname = \"B\"\n")
	for i := 0; i < bundles.SettingMax+1; i++ {
		b.WriteString("[[setting]]\nid = \"b.s")
		b.WriteString(strconv.Itoa(i))
		b.WriteString("\"\nname = \"s")
		b.WriteString(strconv.Itoa(i))
		b.WriteString("\"\n")
	}
	_, err := bundles.ParseManifestFull(dir, []byte(b.String()))
	if err == nil || !strings.Contains(err.Error(), "at most") {
		t.Fatalf("err=%v", err)
	}
}

func TestDiscoverSettingUserOverrides(t *testing.T) {
	user := t.TempDir()
	def := t.TempDir()
	write := func(root, body string) {
		dir := filepath.Join(root, "Pack")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "bundle.toml"), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(def, `
[bundle]
id = "acme.pack"
name = "Pack"
[[setting]]
id = "acme.pack.token"
name = "token"
label = "Shipped"
`)
	write(user, `
[bundle]
id = "acme.pack"
name = "Pack"
[[setting]]
id = "acme.pack.token"
name = "token"
label = "User"
secret = true
`)
	cat, err := bundles.DiscoverCatalog(bundles.DiscoverRoots{UserDir: user, DefaultDir: def})
	if err != nil {
		t.Fatal(err)
	}
	s, ok := bundles.FindSettingByID(cat.Settings, "acme.pack.token")
	if !ok {
		t.Fatal("missing setting")
	}
	if s.Label != "User" || !s.Secret || s.SourceRoot != "user" {
		t.Fatalf("override=%+v", s)
	}
}

func TestShippedLinearSettings(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("caller")
	}
	dir := filepath.Join(filepath.Dir(thisFile), "../../bundles/Linear")
	data, err := os.ReadFile(filepath.Join(dir, "bundle.toml"))
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := bundles.ParseManifestFull(dir, data)
	if err != nil {
		t.Fatal(err)
	}
	if len(parsed.Settings) != 2 {
		t.Fatalf("settings=%d", len(parsed.Settings))
	}
	api, ok := bundles.FindSettingByID(parsed.Settings, "recur.linear.api-key")
	if !ok || api.Name != "api_key" || !api.Secret {
		t.Fatalf("api-key=%+v ok=%v", api, ok)
	}
	team, ok := bundles.FindSettingByID(parsed.Settings, "recur.linear.team-key")
	if !ok || team.Name != "team_key" || team.Secret {
		t.Fatalf("team-key=%+v ok=%v", team, ok)
	}
}
