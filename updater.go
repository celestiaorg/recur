package main

import (
	"context"
	_ "embed"
	"errors"
	"log"
	"net/http"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/updater"
	"github.com/wailsapp/wails/v3/pkg/updater/providers/endpoint"
)

// Stamped by the darwin build via -ldflags -X (APP_LDFLAGS in Taskfile.yml).
// updateManifestURL is only set for release builds (RECUR_UPDATE_URL).
var (
	version           string
	updateManifestURL string
)

// Pins the key release manifests are signed with (UPDATER_SIGNING_KEY secret).
// Wails only checks a signature the manifest carries, so signedOnly rejects
// manifests without one.
//
//go:embed build/updater.key.pub
var updaterPublicKey []byte

// installUpdater wires the headless Wails updater for release .app builds and
// reports whether it did. The frontend renders every state as Recur toasts
// (frontend/src/lib/updater.ts) and asks for a check with "updater.check".
func installUpdater(wapp *application.App) bool {
	if version == "" || updateManifestURL == "" || !runningInAppBundle() {
		return false
	}
	// The default client's 30s Timeout also bounds the body, failing the
	// ~50 MB zip on slow links.
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.ResponseHeaderTimeout = 30 * time.Second
	ep, err := endpoint.New(endpoint.Config{
		URL:        updateManifestURL,
		HTTPClient: &http.Client{Transport: transport},
	})
	if err == nil {
		err = wapp.Updater.Init(updater.Config{
			CurrentVersion: version,
			Providers:      []updater.Provider{signedOnly{ep}},
			PublicKey:      updaterPublicKey,
			Window:         updater.WindowNone,
		})
	}
	if err != nil {
		log.Printf("updater: %v", err)
		return false
	}
	// Not Config.CheckInterval: its loop re-checks from StateReady and
	// re-downloads (discarding the staged update) every tick.
	check := func() {
		switch wapp.Updater.State() {
		case updater.StateIdle, updater.StateUpToDate, updater.StateError:
			_ = wapp.Updater.CheckAndInstall(context.Background())
		}
	}
	wapp.Event.On("updater.check", func(*application.CustomEvent) { go check() })
	go func() {
		for delay := time.Minute; ; delay = 6 * time.Hour {
			time.Sleep(delay)
			check()
		}
	}()
	return true
}

type signedOnly struct{ updater.Provider }

func (p signedOnly) Check(ctx context.Context, req updater.CheckRequest) (*updater.Release, error) {
	rel, err := p.Provider.Check(ctx, req)
	if rel != nil && (rel.Verification == nil || len(rel.Verification.Signature) == 0) {
		return nil, errors.New("updater: release manifest is not signed")
	}
	return rel, err
}
