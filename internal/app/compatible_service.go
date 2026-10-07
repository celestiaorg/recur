package app

import (
	"context"
	"strings"
	"time"

	"recur/internal/domain"
	"recur/internal/responses"
)

// CompatibleTestResult is the Settings Test button outcome for one endpoint.
type CompatibleTestResult struct {
	OK    bool   `json:"ok"`
	Error string `json:"error,omitempty"`
}

// TestCompatibleProvider POSTs a tiny /v1/responses ping using the saved
// (or draft) provider config. Does not persist settings.
func (s *SettingsService) TestCompatibleProvider(p domain.CompatibleProvider) CompatibleTestResult {
	np, err := domain.NormalizeCompatibleProvider(p, nil)
	if err != nil {
		return CompatibleTestResult{Error: err.Error()}
	}
	model := ""
	if len(np.Models) > 0 {
		model = np.Models[0].ID
	}
	if model == "" {
		return CompatibleTestResult{Error: "Add at least one model id first."}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	c := responses.NewClient(np)
	if err := c.Ping(ctx, model); err != nil {
		return CompatibleTestResult{Error: err.Error()}
	}
	return CompatibleTestResult{OK: true}
}

// CompatibleFlagPreset returns stock flag dimensions for the Settings editor.
func (s *SettingsService) CompatibleFlagPreset(name string) []domain.CompatibleFlagDim {
	return domain.CompatibleFlagPreset(strings.TrimSpace(name))
}
