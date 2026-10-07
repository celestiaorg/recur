package app

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	"recur/internal/codex"
	"recur/internal/domain"
	"recur/internal/store"
)

// Codex provider support for the Settings screen. Probes are throwaway
// app-server processes so a bad key / missing binary never wedges a loop.

const codexProbeTimeout = 60 * time.Second

// CodexStatus is what Settings renders for the Codex provider.
// Fields mirror ClaudeStatus: discrete Path / login / email / plan rows.
type CodexStatus struct {
	BinInstalled  bool   `json:"binInstalled"`
	BinPath       string `json:"binPath"`
	Authenticated bool   `json:"authenticated"`
	// LoginMethod is a human-readable auth label (e.g. "API key", "ChatGPT").
	LoginMethod string `json:"loginMethod"`
	// Email is the account email when present (ChatGPT login).
	Email string `json:"email"`
	// Plan is the plan/tier label when present (e.g. "Pro").
	Plan  string `json:"plan"`
	Error string `json:"error"`
}

// GetCodexStatus reports whether Codex is ready without mutating settings.
func (s *SettingsService) GetCodexStatus() CodexStatus {
	set := s.st().Settings()
	bin, err := codex.Locate(set.CodexBinPath)
	if err != nil {
		return CodexStatus{Error: err.Error()}
	}
	out := CodexStatus{BinInstalled: true, BinPath: bin}
	if !set.CodexConfigured() {
		out.Error = "Enable Codex with an API key or ChatGPT auth (Settings → Providers → Codex)."
		return out
	}
	apiKey, useChat := codexAuthChoice(set.CodexAPIKey, set.CodexChatGPTAuth)
	probe, err := s.probeCodex(bin, apiKey, useChat)
	if err != nil {
		out.Error = err.Error()
		return out
	}
	out.Authenticated = true
	out.LoginMethod = probe.LoginMethod
	out.Email = probe.Email
	out.Plan = probe.Plan
	s.rememberCatalog(nil, probe.Models)
	return out
}

// TestCodexKey validates an API key (or ChatGPT auth) before save.
func (s *SettingsService) TestCodexKey(apiKey string, chatGPTAuth bool) CodexStatus {
	set := s.st().Settings()
	if apiKey == "" {
		apiKey = set.CodexAPIKey
	}
	useChat := chatGPTAuth || set.CodexChatGPTAuth
	if apiKey == "" && !useChat {
		return CodexStatus{Error: "Enter an API key or enable ChatGPT auth first."}
	}
	apiKey, useChat = codexAuthChoice(apiKey, useChat)
	bin, err := codex.Locate(set.CodexBinPath)
	if err != nil {
		return CodexStatus{Error: err.Error()}
	}
	probe, err := s.probeCodex(bin, apiKey, useChat)
	if err != nil {
		return CodexStatus{BinInstalled: true, BinPath: bin, Error: err.Error()}
	}
	return CodexStatus{
		BinInstalled:  true,
		BinPath:       bin,
		Authenticated: true,
		LoginMethod:   probe.LoginMethod,
		Email:         probe.Email,
		Plan:          probe.Plan,
	}
}

type codexProbe struct {
	LoginMethod string
	Email       string
	Plan        string
	Models      []codex.Model
}

func (s *SettingsService) probeCodex(bin, apiKey string, chatGPTAuth bool) (codexProbe, error) {
	dataDir, err := store.DataDir()
	if err != nil {
		dataDir = os.TempDir()
	}
	home := codex.HomeDir(dataDir)
	srv := &codex.Server{
		Bin:             bin,
		Home:            home,
		Worktree:        dataDir,
		APIKey:          apiKey,
		ConfigOverrides: []string{`approval_policy="never"`},
	}
	if apiKey == "" && chatGPTAuth {
		srv.SeedAuthFrom = codex.DefaultSeedAuthPath()
	}
	ctx, cancel := context.WithTimeout(context.Background(), codexProbeTimeout)
	defer cancel()
	if err := srv.Start(ctx); err != nil {
		return codexProbe{}, err
	}
	defer srv.Close()

	client := codex.NewClient(srv.Conn())
	acct, err := client.AccountRead(ctx)
	if err != nil {
		return codexProbe{}, fmt.Errorf("account/read: %w", err)
	}
	if acct.Account == nil {
		return codexProbe{}, fmt.Errorf("not authenticated — set an API key or run `codex login`")
	}
	models, _ := client.ModelList(ctx, 50)
	return codexProbe{
		LoginMethod: codexLoginMethod(acct.Account.Type, apiKey != ""),
		Email:       strings.TrimSpace(acct.Account.Email),
		Plan:        codex.FormatPlan(acct.Account.PlanType),
		Models:      models,
	}, nil
}

func codexLoginMethod(accountType string, hasAPIKey bool) string {
	switch strings.ToLower(strings.TrimSpace(accountType)) {
	case "chatgpt", "chat_gpt", "openai":
		return "ChatGPT account"
	case "apikey", "api_key", "api":
		return "API key"
	}
	if hasAPIKey {
		return "API key"
	}
	if accountType != "" {
		return accountType
	}
	return "ChatGPT account"
}

// codexAuthChoice applies ChatGPT-login precedence: when the toggle is on, a
// leftover API key is ignored so Settings' hidden key field cannot keep winning.
func codexAuthChoice(apiKey string, chatGPTAuth bool) (key string, useChat bool) {
	if chatGPTAuth {
		return "", true
	}
	return strings.TrimSpace(apiKey), false
}

func (s *SettingsService) listCodexModelsFor(set domain.Settings) []codex.Model {
	if !set.ProviderEnabled(domain.ProviderCodex) || !set.CodexConfigured() {
		return nil
	}
	bin, err := codex.Locate(set.CodexBinPath)
	if err != nil {
		return nil
	}
	dataDir, _ := store.DataDir()
	if dataDir == "" {
		dataDir = os.TempDir()
	}
	home := codex.HomeDir(dataDir)
	apiKey, useChat := codexAuthChoice(set.CodexAPIKey, set.CodexChatGPTAuth)
	srv := &codex.Server{
		Bin:             bin,
		Home:            home,
		Worktree:        dataDir,
		APIKey:          apiKey,
		ConfigOverrides: []string{`approval_policy="never"`},
	}
	if useChat {
		srv.SeedAuthFrom = codex.DefaultSeedAuthPath()
	}
	ctx, cancel := context.WithTimeout(context.Background(), codexProbeTimeout)
	defer cancel()
	if err := srv.Start(ctx); err != nil {
		return nil
	}
	defer srv.Close()
	models, err := codex.NewClient(srv.Conn()).ModelList(ctx, 50)
	if err != nil {
		return nil
	}
	return models
}
