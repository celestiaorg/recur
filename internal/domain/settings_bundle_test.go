package domain

import "testing"

func TestMigrateBundleValues(t *testing.T) {
	s := Settings{LinearAPIKey: "lin_old", LinearTeamKey: "ENG"}
	got := s.MigrateBundleValues()
	if got.LinearAPIKey != "" || got.LinearTeamKey != "" {
		t.Fatalf("old fields still set: %+v", got)
	}
	if got.BundleValues[SettingLinearAPIKey] != "lin_old" {
		t.Fatalf("api key=%q", got.BundleValues[SettingLinearAPIKey])
	}
	if got.BundleValues[SettingLinearTeamKey] != "ENG" {
		t.Fatalf("team=%q", got.BundleValues[SettingLinearTeamKey])
	}
	// Existing BundleValues win.
	s = Settings{
		LinearAPIKey: "old",
		BundleValues: map[string]string{SettingLinearAPIKey: "new"},
	}
	got = s.MigrateBundleValues()
	if got.BundleValues[SettingLinearAPIKey] != "new" {
		t.Fatalf("did not keep stored value: %q", got.BundleValues[SettingLinearAPIKey])
	}
	if got.BundleValue(SettingLinearAPIKey) != "new" {
		t.Fatalf("BundleValue=%q", got.BundleValue(SettingLinearAPIKey))
	}
}
