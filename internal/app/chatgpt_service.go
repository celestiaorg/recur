package app

import (
	"context"
	"fmt"
	"time"

	"recur/internal/chatgpt"
	"recur/internal/domain"
)

// ChatGPTStatus exposes account metadata only, never OAuth tokens.
type ChatGPTStatus struct {
	chatgpt.Status
	ModelCount int    `json:"modelCount"`
	Error      string `json:"error"`
}

func (s *SettingsService) GetChatGPTStatus() ChatGPTStatus {
	c, err := s.eng.ChatGPTClient()
	if err != nil {
		return ChatGPTStatus{Error: err.Error()}
	}
	status, err := c.Status()
	if err != nil {
		return ChatGPTStatus{Error: err.Error()}
	}
	if status.Authenticated {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		if _, err := c.AccessToken(ctx, status.AccountID); err != nil {
			status.Authenticated = false
			return ChatGPTStatus{Status: status, Error: err.Error()}
		}
	}
	set := s.st().Settings()
	count := 0
	if status.Authenticated && status.AccountID == set.ChatGPTAccountID {
		count = len(set.ChatGPTModels)
	}
	return ChatGPTStatus{Status: status, ModelCount: count}
}

// StartChatGPTLogin starts Codex-compatible browser or headless sign-in.
// An empty accountID allows a new account; a saved ID pins the selected account.
func (s *SettingsService) StartChatGPTLogin(accountID, method string) (chatgpt.Login, error) {
	if !s.st().Settings().ProviderEnabled(domain.ProviderChatGPT) {
		return chatgpt.Login{}, fmt.Errorf("enable ChatGPT before signing in")
	}
	c, err := s.eng.ChatGPTClient()
	if err != nil {
		return chatgpt.Login{}, err
	}
	return c.StartLogin(accountID, method)
}

func (s *SettingsService) WaitChatGPTLogin(loginID string) (ChatGPTStatus, error) {
	c, err := s.eng.ChatGPTClient()
	if err != nil {
		return ChatGPTStatus{}, err
	}
	if err := c.WaitLogin(loginID); err != nil {
		return ChatGPTStatus{}, err
	}
	return s.RefreshChatGPTModels()
}

func (s *SettingsService) CancelChatGPTLogin(loginID string) {
	if c, err := s.eng.ChatGPTClient(); err == nil {
		c.CancelLogin(loginID)
	}
}

// RefreshChatGPTModels queries the signed-in ChatGPT Codex model catalogue.
func (s *SettingsService) RefreshChatGPTModels() (ChatGPTStatus, error) {
	c, err := s.eng.ChatGPTClient()
	if err != nil {
		return ChatGPTStatus{}, err
	}
	status, err := c.Status()
	if err != nil {
		return ChatGPTStatus{}, err
	}
	if !status.Authenticated {
		return ChatGPTStatus{}, fmt.Errorf("continue with ChatGPT to sign in")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	models, err := c.Models(ctx, status.AccountID)
	if err != nil {
		return ChatGPTStatus{}, err
	}
	current, err := c.Status()
	if err != nil {
		return ChatGPTStatus{}, err
	}
	if current.AccountID != status.AccountID || !current.Authenticated {
		return ChatGPTStatus{}, fmt.Errorf("ChatGPT account changed while loading models. Refresh models again")
	}
	set := s.st().Settings()
	set.ChatGPTAccountID = status.AccountID
	set.ChatGPTModels = nil
	for _, m := range models {
		set.ChatGPTModels = append(set.ChatGPTModels, domain.CompatibleModel{ID: m.ID, Label: m.Name})
	}
	if _, err := s.SetSettings(set); err != nil {
		return ChatGPTStatus{}, err
	}
	return ChatGPTStatus{Status: status, ModelCount: len(models)}, nil
}

func (s *SettingsService) DisconnectChatGPT() (ChatGPTStatus, error) {
	c, err := s.eng.ChatGPTClient()
	if err != nil {
		return ChatGPTStatus{}, err
	}
	if err := c.Disconnect(); err != nil {
		return ChatGPTStatus{}, err
	}
	set := s.st().Settings()
	set.ChatGPTAccountID, set.ChatGPTModels = "", nil
	if _, err := s.SetSettings(set); err != nil {
		return ChatGPTStatus{}, err
	}
	return s.GetChatGPTStatus(), nil
}
