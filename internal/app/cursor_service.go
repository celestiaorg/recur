package app

import (
	"context"
	"errors"
	"fmt"
	"log"
	"time"

	"recur/internal/cursor"
	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
	"recur/internal/usage"
)

// Cursor provider support for the Settings screen. These calls are deliberately
// independent of the engine's long-lived bridge: each spins up a throwaway
// bridge so the user can validate a key BEFORE saving it, and so a bad key can
// never wedge the running one.

// cursorProbeTimeout bounds a Settings probe. Generous because the first bridge
// start pays a ~117MB Node cold start.
const cursorProbeTimeout = 120 * time.Second

// CursorStatus is what the Settings screen renders for the Cursor provider.
type CursorStatus struct {
	// BridgeInstalled reports whether a usable cursor-sdk-bridge was found.
	BridgeInstalled bool `json:"bridgeInstalled"`
	// BridgePath is the resolved launcher path ("" when not found).
	BridgePath string `json:"bridgePath"`
	// BridgeVersion is the installed cursor-sdk pin from manifest.json
	// (preferred over GetVersion's bridge shell version, which stays at 1.0.0).
	BridgeVersion string `json:"bridgeVersion"`
	// PinnedVersion is the cursor-sdk release Recur would Install.
	PinnedVersion string `json:"pinnedVersion"`
	// UpdateAvailable is true when Recur's dataDir bridge is behind
	// PinnedVersion, so Settings can surface Reinstall as an update.
	UpdateAvailable bool `json:"updateAvailable"`
	// Authenticated reports whether the configured API key works.
	Authenticated bool `json:"authenticated"`
	// Account is a short identity for toasts (email or API key name).
	Account string `json:"account"`
	// APIKeyName is the dashboard name of the key (from Cursor.me).
	APIKeyName string `json:"apiKeyName"`
	// Email is the account email when Cursor.me returns one.
	Email string `json:"email"`
	// Name is the account display name when Cursor.me returns one.
	Name string `json:"name"`
	// ModelCount is how many models the key may use.
	ModelCount int `json:"modelCount"`
	// Error is a user-facing explanation when something is not ready.
	Error string `json:"error"`
}

// GetCursorStatus reports whether the Cursor provider is ready to use.
// A leftover pasted key is dropped when Recur has no Connect file. Safe to
// call on every Settings render: when no bridge is installed it does no work
// beyond a few Stat calls (and that orphan wipe).
func (s *SettingsService) GetCursorStatus() CursorStatus {
	set := s.st().Settings()
	dataDir, _ := store.DataDir()
	set = s.dropOrphanCursorKey(set, dataDir)

	bin, err := cursor.Locate(dataDir)
	if err != nil {
		return CursorStatus{Error: err.Error()}
	}
	out := CursorStatus{
		BridgeInstalled: true,
		BridgePath:      bin,
		BridgeVersion:   cursor.SDKVersion(bin),
	}

	if !usage.HasRecurCursorAuthFile(dataDir) || set.CursorAPIKey == "" {
		applyCursorPin(&out, dataDir, bin)
		return out
	}

	probe, err := s.probeCursor(bin, set.CursorAPIKey)
	if err != nil {
		out.Error = err.Error()
		applyCursorPin(&out, dataDir, bin)
		return out
	}
	applyCursorProbe(&out, probe)
	s.rememberCatalog(probe.Models, nil)
	applyCursorPin(&out, dataDir, bin)
	return out
}

// TestCursorKey validates an API key that has NOT been saved yet, so the user
// gets a verdict before committing it. An empty key falls back to the saved one.
func (s *SettingsService) TestCursorKey(apiKey string) CursorStatus {
	set := s.st().Settings()
	if apiKey == "" {
		apiKey = set.CursorAPIKey
	}
	if apiKey == "" {
		return CursorStatus{Error: "Enter an API key first."}
	}

	dataDir, _ := store.DataDir()
	bin, err := cursor.Locate(dataDir)
	if err != nil {
		return CursorStatus{Error: err.Error()}
	}

	probe, err := s.probeCursor(bin, apiKey)
	if err != nil {
		out := CursorStatus{
			BridgeInstalled: true,
			BridgePath:      bin,
			BridgeVersion:   cursor.SDKVersion(bin),
			Error:           err.Error(),
		}
		applyCursorPin(&out, dataDir, bin)
		return out
	}
	out := CursorStatus{
		BridgeInstalled: true,
		BridgePath:      bin,
		BridgeVersion:   cursor.SDKVersion(bin),
	}
	applyCursorProbe(&out, probe)
	applyCursorPin(&out, dataDir, bin)
	return out
}

// listCursorModelsFor returns the models the configured key may use. Returns nil
// (not an error) when Cursor is not configured, so the catalogue simply omits
// the group. Callers pass the settings blob to use so a live probe during
// SetSettings (cold catalogue cache) sees the incoming key, not the still-old store.
func (s *SettingsService) listCursorModelsFor(set domain.Settings) []cursor.Model {
	if !set.ProviderEnabled(domain.ProviderCursor) || !set.CursorConfigured() {
		return nil
	}
	dataDir, _ := store.DataDir()
	bin, err := cursor.Locate(dataDir)
	if err != nil {
		return nil
	}

	ctx, cancel := context.WithTimeout(context.Background(), cursorProbeTimeout)
	defer cancel()

	b := &cursor.Bridge{Bin: bin, APIKey: set.CursorAPIKey}
	if err := b.Start(ctx); err != nil {
		return nil
	}
	defer func() { _ = b.Close() }()

	models, err := b.Client().ListModels(ctx, set.CursorAPIKey)
	if err != nil {
		return nil
	}
	return models
}

// InstallCursorBridge downloads and unpacks the Cursor bridge into Recur's data
// directory, then records the resolved path in Settings.
//
// The bridge is ~50MB compressed / ~155MB unpacked (mostly a bundled Node
// runtime), so it is fetched on demand rather than shipped with Recur. Progress
// is published on the bus as `cursor.install` events; this call blocks until the
// install finishes or fails.
func (s *SettingsService) InstallCursorBridge() (CursorStatus, error) {
	dataDir, err := store.DataDir()
	if err != nil {
		return CursorStatus{Error: err.Error()}, err
	}

	bin, err := cursor.Install(context.Background(), dataDir, func(p cursor.InstallProgress) {
		s.eng.Bus.Publish(engine.Event{Name: "cursor.install", Data: map[string]any{
			"phase":   string(p.Phase),
			"message": p.Message,
			"current": p.Current,
			"total":   p.Total,
			"percent": p.Percent(),
		}})
	})
	if err != nil {
		s.eng.Bus.Publish(engine.Event{Name: "cursor.install", Data: map[string]any{
			"phase": "error", "message": err.Error(), "percent": -1,
		}})
		return CursorStatus{Error: err.Error()}, err
	}

	// Record the launcher path in Settings (display/history only; Locate
	// always uses <dataDir>/cursor-bridge).
	set := s.st().Settings()
	set.CursorBridgePath = bin
	s.st().SetSettings(set)
	if werr := cursor.WriteAttributionConfig(
		cursor.AttributionConfigPath(dataDir),
		set.CursorAttributeCommits,
		set.CursorAttributePRs,
	); werr != nil {
		log.Printf("recur: warning: write cursor attribution config: %v", werr)
	}

	return s.GetCursorStatus(), nil
}

// cursorProbe is the result of one throwaway bridge round-trip.
type cursorProbe struct {
	BridgeVersion string
	Account       string
	APIKeyName    string
	Email         string
	Name          string
	ModelCount    int
	Models        []cursor.Model
}

func applyCursorPin(out *CursorStatus, dataDir, bin string) {
	out.PinnedVersion = cursor.PinnedSDKVersion()
	if cursor.OwnedBridgeRoot(dataDir, bin) == "" {
		out.UpdateAvailable = false
		return
	}
	out.UpdateAvailable = out.BridgeVersion != "" && out.BridgeVersion != out.PinnedVersion
}

func applyCursorProbe(out *CursorStatus, probe cursorProbe) {
	out.BridgeVersion = probe.BridgeVersion
	out.Authenticated = true
	out.Account = probe.Account
	out.APIKeyName = probe.APIKeyName
	out.Email = probe.Email
	out.Name = probe.Name
	out.ModelCount = probe.ModelCount
}

// probeCursor starts a temporary bridge and exercises the credentialed path.
func (s *SettingsService) probeCursor(bin, apiKey string) (cursorProbe, error) {
	ctx, cancel := context.WithTimeout(context.Background(), cursorProbeTimeout)
	defer cancel()

	b := &cursor.Bridge{Bin: bin, APIKey: apiKey}
	if err := b.Start(ctx); err != nil {
		return cursorProbe{}, fmt.Errorf("could not start the Cursor bridge: %w", err)
	}
	defer func() { _ = b.Close() }()

	if _, err := b.Client().GetVersion(ctx); err != nil {
		return cursorProbe{}, fmt.Errorf("bridge handshake failed: %w", err)
	}

	// ListModels is the cheapest credentialed call, so it doubles as the key check.
	models, err := b.Client().ListModels(ctx, apiKey)
	if err != nil {
		var cerr *cursor.Error
		if errors.As(err, &cerr) && cerr.Unauthenticated() {
			return cursorProbe{}, fmt.Errorf("that API key was rejected by Cursor")
		}
		return cursorProbe{}, err
	}

	// Prefer manifest sdkVersion (the pin Recur installs) over GetVersion's
	// bridge shell version, which is currently always "1.0.0".
	ver := cursor.SDKVersion(bin)
	out := cursorProbe{BridgeVersion: ver, ModelCount: len(models), Models: models}
	// Identity is a nice-to-have; a key that lists models is already valid, so a
	// failure here must not mark the provider broken.
	if me, err := b.Client().Me(ctx, apiKey); err == nil {
		out.APIKeyName = me.APIKeyName
		out.Email = me.UserEmail
		out.Name = me.DisplayName()
		out.Account = me.Summary()
	}
	return out, nil
}
