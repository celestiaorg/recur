package app

import (
	"os"
	"path/filepath"
	"testing"

	"recur/internal/usage"
)

func TestGetProviderUsageWithoutPoller(t *testing.T) {
	s := &SettingsService{}
	got := s.GetProviderUsage()
	if got.Providers == nil {
		t.Fatal("GetProviderUsage() Providers = nil, want empty slice")
	}
	if len(got.Providers) != 0 {
		t.Errorf("GetProviderUsage() = %+v, want empty", got)
	}
	conn := s.ConnectCursorUsage()
	if conn.Error == "" {
		t.Fatal("ConnectCursorUsage without poller should error")
	}
	s.CancelCursorUsageConnect()
	if err := s.DisconnectCursorUsage(); err != nil {
		t.Errorf("DisconnectCursorUsage() = %v", err)
	}
	_ = usage.EmptySnapshot()
}

func TestSetSettingsTurningCursorOffClearsKey(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.st().Settings()
	set.CursorAPIKey = "crsr_on"
	mustSetSettings(t, svc.Settings, set)
	set = svc.Settings.st().Settings()
	set.DisabledProviders = append(set.DisabledProviders, "cursor")
	got := mustSetSettings(t, svc.Settings, set)
	if got.CursorAPIKey != "" {
		t.Errorf("SetSettings(cursor off) CursorAPIKey = %q, want empty", got.CursorAPIKey)
	}
	if svc.Settings.st().Settings().CursorAPIKey != "" {
		t.Error("store still has Cursor API key after turning Cursor off")
	}
}

func TestDisconnectCursorUsageClearsSettingsKey(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.st().Settings()
	set.CursorAPIKey = "crsr_leftover"
	mustSetSettings(t, svc.Settings, set)
	if err := svc.Settings.DisconnectCursorUsage(); err != nil {
		t.Fatalf("DisconnectCursorUsage() = %v", err)
	}
	if got := svc.Settings.st().Settings().CursorAPIKey; got != "" {
		t.Errorf("CursorAPIKey = %q, want empty after disconnect", got)
	}
}

func TestDropOrphanCursorKey(t *testing.T) {
	svc, _ := newTestServices(t)
	set := svc.Settings.st().Settings()
	set.CursorAPIKey = "crsr_leftover"
	mustSetSettings(t, svc.Settings, set)

	got := svc.Settings.dropOrphanCursorKey(svc.Settings.st().Settings(), t.TempDir())
	if got.CursorAPIKey != "" {
		t.Errorf("dropOrphanCursorKey() key = %q, want empty", got.CursorAPIKey)
	}
	if svc.Settings.st().Settings().CursorAPIKey != "" {
		t.Error("store still has leftover key")
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "cursor-usage-auth.json")
	if err := os.WriteFile(path, []byte(`{"accessToken":"sess"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	set = svc.Settings.st().Settings()
	set.CursorAPIKey = "crsr_kept"
	mustSetSettings(t, svc.Settings, set)
	got = svc.Settings.dropOrphanCursorKey(svc.Settings.st().Settings(), dir)
	if got.CursorAPIKey != "crsr_kept" {
		t.Errorf("dropOrphanCursorKey() with Connect file key = %q, want kept", got.CursorAPIKey)
	}
}
