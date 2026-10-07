package app

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"recur/internal/domain"
)

func TestTestCompatibleProviderHitsResponses(t *testing.T) {
	var path string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path = r.URL.Path
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"id": "r", "status": "completed", "output": []any{}})
	}))
	t.Cleanup(srv.Close)

	got := (&SettingsService{}).TestCompatibleProvider(domain.CompatibleProvider{
		ID:      "openai",
		BaseURL: srv.URL,
		Models:  []domain.CompatibleModel{{ID: "gpt-5"}},
	})
	if !got.OK {
		t.Errorf("TestCompatibleProvider() = %+v, want ok", got)
	}
	if path != "/v1/responses" {
		t.Errorf("Ping path = %q, want /v1/responses", path)
	}
}

func TestTestCompatibleProviderNeedsModel(t *testing.T) {
	got := (&SettingsService{}).TestCompatibleProvider(domain.CompatibleProvider{
		ID: "openai", BaseURL: "https://api.openai.com",
	})
	if got.OK || got.Error == "" {
		t.Errorf("TestCompatibleProvider(no models) = %+v, want error", got)
	}
}
