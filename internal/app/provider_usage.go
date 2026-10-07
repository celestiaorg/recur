package app

import (
	"log"
	"strings"

	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
	"recur/internal/usage"
)

// StartProviderUsage starts the background quota poller. Call from main after
// NewServices — tests that only construct services must not hit provider APIs.
// This is a package function (not a bound method) so the renderer cannot stop
// or start the poller.
func StartProviderUsage(s *SettingsService) {
	if s == nil {
		return
	}
	s.usageMu.Lock()
	defer s.usageMu.Unlock()
	if s.usagePoller != nil {
		return
	}
	fetch := &usage.Fetcher{
		DataDir:        store.DataDir,
		Want:           s.usageWants,
		CursorStub:     s.usageCursorStub,
		OnMintedAPIKey: s.applyMintedCursorKey,
	}
	p := usage.NewPoller(fetch, func(name string, data any) {
		s.eng.Bus.Publish(engine.Event{Name: name, Data: data})
	})
	s.usagePoller = p
	p.Start()
}

// StopProviderUsage stops the ticker and any in-flight Cursor Connect.
func StopProviderUsage(s *SettingsService) {
	if s == nil {
		return
	}
	s.usageMu.Lock()
	p := s.usagePoller
	s.usagePoller = nil
	s.usageMu.Unlock()
	if p != nil {
		p.Stop()
	}
}

func (s *SettingsService) usageWants() (claude, codex, cursor bool) {
	if s.eng == nil {
		return false, false, false
	}
	set := s.st().Settings()
	dir := ""
	if d, err := store.DataDir(); err == nil {
		dir = d
	}
	claude = set.ProviderEnabled(domain.ProviderClaudeCode)
	codex = set.ProviderEnabled(domain.ProviderCodex) && set.CodexChatGPTAuth
	// Fetch only when Cursor is on and Recur has a Connect session. The crsr_
	// API key cannot read dashboard usage.
	cursor = set.ProviderEnabled(domain.ProviderCursor) && usage.HasRecurCursorAuthFile(dir)
	return
}

func (s *SettingsService) usageCursorStub() *usage.ProviderQuota {
	if s.eng == nil {
		return nil
	}
	set := s.st().Settings()
	dir := ""
	if d, err := store.DataDir(); err == nil {
		dir = d
	}
	if usage.HasRecurCursorAuthFile(dir) {
		q := usage.CursorConnectTokensQuota()
		return &q
	}
	if set.ProviderEnabled(domain.ProviderCursor) {
		q := usage.CursorNoSessionQuota()
		return &q
	}
	return nil
}

func (s *SettingsService) poller() *usage.Poller {
	if s == nil {
		return nil
	}
	s.usageMu.Lock()
	defer s.usageMu.Unlock()
	return s.usagePoller
}

// GetProviderUsage returns the last quota snapshot, refreshing if it is stale.
func (s *SettingsService) GetProviderUsage() usage.Snapshot {
	if p := s.poller(); p != nil {
		return p.Snapshot(false)
	}
	return usage.EmptySnapshot()
}

// RefreshProviderUsage forces a fetch, joining any in-flight collect.
func (s *SettingsService) RefreshProviderUsage() usage.Snapshot {
	if p := s.poller(); p != nil {
		return p.Snapshot(true)
	}
	return usage.EmptySnapshot()
}

// ConnectCursorUsage starts the unofficial Cursor browser login and returns the URL.
func (s *SettingsService) ConnectCursorUsage() usage.CursorUsageConnect {
	if p := s.poller(); p != nil {
		return p.ConnectCursor()
	}
	return usage.CursorUsageConnect{Error: "Usage poller is not running"}
}

// CancelCursorUsageConnect aborts an in-flight Connect Cursor login.
func (s *SettingsService) CancelCursorUsageConnect() {
	if p := s.poller(); p != nil {
		p.CancelConnect()
	}
}

// DisconnectCursorUsage deletes Recur-owned Cursor Connect tokens and the
// minted SDK key. A leftover pasted key is also cleared — Connect is the
// only Cursor sign-in.
func (s *SettingsService) DisconnectCursorUsage() error {
	err := s.revokeCursorConnect()
	s.clearCursorAPIKey()
	return err
}

func (s *SettingsService) revokeCursorConnect() error {
	if p := s.poller(); p != nil {
		return p.DisconnectCursor()
	}
	return nil
}

func (s *SettingsService) clearCursorAPIKey() {
	if s == nil || s.eng == nil {
		return
	}
	set := s.st().Settings()
	if strings.TrimSpace(set.CursorAPIKey) == "" {
		return
	}
	set.CursorAPIKey = ""
	if _, err := s.SetSettings(set); err != nil {
		log.Printf("recur: warning: clear cursor api key: %v", err)
	}
}

// dropOrphanCursorKey clears a Settings key when Recur has no Connect file.
// GetCursorStatus calls this so a leftover paste cannot keep showing Connected.
func (s *SettingsService) dropOrphanCursorKey(set domain.Settings, dataDir string) domain.Settings {
	if usage.HasRecurCursorAuthFile(dataDir) {
		return set
	}
	if strings.TrimSpace(set.CursorAPIKey) == "" {
		return set
	}
	s.clearCursorAPIKey()
	set.CursorAPIKey = ""
	return set
}

// applyMintedCursorKey writes a Connect-minted user API key into Settings.
func (s *SettingsService) applyMintedCursorKey(_, next string) {
	next = strings.TrimSpace(next)
	if next == "" || s == nil || s.eng == nil {
		return
	}
	set := s.st().Settings()
	if strings.TrimSpace(set.CursorAPIKey) == next {
		return
	}
	set.CursorAPIKey = next
	if _, err := s.SetSettings(set); err != nil {
		log.Printf("recur: warning: save cursor api key: %v", err)
	}
}
