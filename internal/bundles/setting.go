package bundles

import (
	"fmt"
	"strings"
)

// SettingMax is the per-bundle ceiling for [[setting]] entries.
const SettingMax = 16

// Setting is one [[setting]] entry — a persisted value Recur shows in
// Settings → Integrations and injects into that bundle's shebang env as
// RECUR_SETTING_<NAME> plus RECUR_BUNDLE_SETTINGS JSON. TypeScript Workers
// never see these values.
type Setting struct {
	ID          string `toml:"id" json:"id"`
	Name        string `toml:"name" json:"name"` // machine name → RECUR_SETTING_*
	Label       string `toml:"label" json:"label"`
	Description string `toml:"description" json:"description"`
	Secret      bool   `toml:"secret" json:"secret"`
	Placeholder string `toml:"placeholder" json:"placeholder"`
	URL         string `toml:"url" json:"url"`
	URLLabel    string `toml:"urlLabel" json:"urlLabel"`

	// Filled by Discover — not part of the TOML schema.
	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

// SettingEnvName is the shebang helper for a [[setting]] name (api_key → RECUR_SETTING_API_KEY).
func SettingEnvName(name string) string {
	return "RECUR_SETTING_" + strings.ToUpper(strings.TrimSpace(name))
}

func parseSettings(bundleDir string, meta BundleMeta, raw []Setting) ([]Setting, error) {
	if len(raw) > SettingMax {
		return nil, fmt.Errorf("setting: at most %d entries per bundle", SettingMax)
	}
	out := make([]Setting, 0, len(raw))
	seenID := map[string]struct{}{}
	seenName := map[string]struct{}{}
	for i, s := range raw {
		id := strings.TrimSpace(s.ID)
		if id == "" {
			return nil, fmt.Errorf("setting[%d]: id is required", i)
		}
		if !validBundleID(id) {
			return nil, fmt.Errorf("setting[%d]: id is not a safe id", i)
		}
		if _, dup := seenID[id]; dup {
			return nil, fmt.Errorf("setting: duplicate id %q in bundle", id)
		}
		seenID[id] = struct{}{}
		s.ID = id

		name := strings.TrimSpace(s.Name)
		if name == "" {
			return nil, fmt.Errorf("setting %q: name is required", id)
		}
		if !promptArgNameRE.MatchString(name) {
			return nil, fmt.Errorf("setting %q: name %q must match %s", id, name, promptArgNameRE.String())
		}
		if _, dup := seenName[name]; dup {
			return nil, fmt.Errorf("setting: duplicate name %q in bundle", name)
		}
		seenName[name] = struct{}{}
		s.Name = name

		s.Label = strings.TrimSpace(s.Label)
		if s.Label == "" {
			s.Label = name
		}
		s.Description = strings.TrimSpace(s.Description)
		s.Placeholder = strings.TrimSpace(s.Placeholder)
		s.URL = strings.TrimSpace(s.URL)
		s.URLLabel = strings.TrimSpace(s.URLLabel)
		s.BundleName = meta.Name
		s.BundleID = meta.ID
		s.BundleDir = bundleDir
		out = append(out, s)
	}
	return out, nil
}

// FindSettingByID returns the setting with the given id.
func FindSettingByID(all []Setting, id string) (Setting, bool) {
	for _, s := range all {
		if s.ID == id {
			return s, true
		}
	}
	return Setting{}, false
}

// SettingsForBundle returns [[setting]] entries belonging to bundleID.
func SettingsForBundle(all []Setting, bundleID string) []Setting {
	out := make([]Setting, 0)
	for _, s := range all {
		if s.BundleID == bundleID {
			out = append(out, s)
		}
	}
	return out
}
