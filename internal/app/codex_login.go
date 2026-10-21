package app

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/google/uuid"

	"recur/internal/codex"
	"recur/internal/domain"
	"recur/internal/store"
)

const codexLoginTimeout = 5 * time.Minute

// CodexLogin identifies a pending sign-in and the URL to open in the OS browser.
type CodexLogin struct {
	LoginID string `json:"loginId"`
	AuthURL string `json:"authUrl"`
}

type codexLoginStart struct {
	login CodexLogin
	err   error
}

type codexLoginSession struct {
	id      string
	ctx     context.Context
	cancel  context.CancelFunc
	started chan codexLoginStart
	done    chan struct{}
	// Written by the worker before closing done.
	status CodexStatus
	models []codex.Model
	err    error
}

// StartCodexLogin starts a Recur-owned ChatGPT browser sign-in. No CLI login is needed.
func (s *SettingsService) StartCodexLogin() (CodexLogin, error) {
	set := s.st().Settings()
	if !set.ProviderEnabled(domain.ProviderCodex) {
		return CodexLogin{}, fmt.Errorf("enable Codex before signing in")
	}
	bin, err := codex.Locate(set.CodexBinPath)
	if err != nil {
		return CodexLogin{}, err
	}
	dataDir, err := store.DataDir()
	if err != nil {
		return CodexLogin{}, err
	}
	s.codexLoginMu.Lock()
	if s.codexLoginClosed {
		s.codexLoginMu.Unlock()
		return CodexLogin{}, fmt.Errorf("Recur is shutting down")
	}
	if prev := s.codexLogin; prev != nil {
		select {
		case <-prev.done:
		default:
			s.codexLoginMu.Unlock()
			return CodexLogin{}, fmt.Errorf("Codex sign-in is already in progress")
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), codexLoginTimeout)
	session := &codexLoginSession{
		id: uuid.NewString(), ctx: ctx, cancel: cancel,
		started: make(chan codexLoginStart, 1), done: make(chan struct{}),
	}
	s.codexLogin = session
	s.codexLoginMu.Unlock()
	go session.run(bin, dataDir)
	result := <-session.started
	return result.login, result.err
}

// WaitCodexLogin waits for browser sign-in, then selects the signed-in ChatGPT account.
func (s *SettingsService) WaitCodexLogin(loginID string) (CodexStatus, error) {
	s.codexLoginMu.Lock()
	session := s.codexLogin
	s.codexLoginMu.Unlock()
	if session == nil || session.id != loginID {
		return CodexStatus{}, fmt.Errorf("Codex sign-in is no longer pending")
	}
	<-session.done
	s.codexLoginMu.Lock()
	defer s.codexLoginMu.Unlock()
	if s.codexLogin != session {
		return CodexStatus{}, context.Canceled
	}
	s.codexLogin = nil
	if session.err != nil {
		return CodexStatus{}, session.err
	}
	set := s.st().Settings()
	if !set.ProviderEnabled(domain.ProviderCodex) {
		return CodexStatus{}, fmt.Errorf("Codex was disabled during sign-in")
	}
	set.CodexChatGPTAuth = true
	set.CodexAPIKey = ""
	s.rememberCatalog(nil, session.models)
	if _, err := s.SetSettings(set); err != nil {
		return CodexStatus{}, err
	}
	return session.status, nil
}

// CancelCodexLogin cancels only the specified attempt, leaving newer logins alone.
func (s *SettingsService) CancelCodexLogin(loginID string) {
	s.codexLoginMu.Lock()
	defer s.codexLoginMu.Unlock()
	session := s.codexLogin
	if session == nil || session.id != loginID {
		return
	}
	s.codexLogin = nil
	session.cancel()
	// Serialize reaping with retries and shutdown so the old callback listener
	// is gone before another attempt can start.
	<-session.done
}

// ServiceShutdown joins the login worker so its callback listener cannot outlive Recur.
func (s *SettingsService) ServiceShutdown() error {
	if s.eng != nil {
		s.eng.CloseChatGPT()
	}
	s.codexLoginMu.Lock()
	s.codexLoginClosed = true
	session := s.codexLogin
	s.codexLogin = nil
	if session != nil {
		session.cancel()
	}
	s.codexLoginMu.Unlock()
	if session != nil {
		<-session.done
	}
	return nil
}

func (s *codexLoginSession) run(bin, dataDir string) {
	announced := false
	defer func() {
		if s.ctx.Err() == context.DeadlineExceeded {
			s.err = fmt.Errorf("Codex sign-in timed out. Try again")
		}
		s.cancel()
		close(s.done)
		if !announced {
			s.started <- codexLoginStart{err: s.err}
		}
	}()
	srv := &codex.Server{
		Bin: bin, Home: codex.HomeDir(dataDir), Worktree: dataDir,
		// Persist in this CODEX_HOME, independent of any global keyring config.
		ConfigOverrides: []string{`cli_auth_credentials_store="file"`},
	}
	if s.err = srv.Start(s.ctx); s.err != nil {
		return
	}
	defer srv.Close()
	client := codex.NewClient(srv.Conn())
	completed := make(chan codex.LoginCompleted, 1)
	srv.Conn().OnNotification(func(method string, params json.RawMessage) {
		if method != "account/login/completed" {
			return
		}
		var result codex.LoginCompleted
		if json.Unmarshal(params, &result) == nil {
			select {
			case completed <- result:
			default:
			}
		}
	})
	login, err := client.AccountLoginStart(s.ctx)
	if err != nil {
		s.err = fmt.Errorf("start Codex sign-in: %w", err)
		return
	}
	if login.LoginID == "" || login.AuthURL == "" {
		s.err = fmt.Errorf("Codex did not return a browser sign-in URL")
		return
	}
	s.started <- codexLoginStart{login: CodexLogin{LoginID: s.id, AuthURL: login.AuthURL}}
	announced = true
	for {
		select {
		case <-s.ctx.Done():
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			_ = client.AccountLoginCancel(ctx, login.LoginID)
			cancel()
			s.err = s.ctx.Err()
			return
		case <-srv.Reaped():
			s.err = fmt.Errorf("Codex exited during sign-in")
			return
		case result := <-completed:
			if result.LoginID != login.LoginID {
				continue
			}
			if !result.Success {
				s.err = fmt.Errorf("Codex sign-in failed: %s", result.Error)
				return
			}
			acct, err := client.AccountRead(s.ctx)
			if err != nil {
				s.err = err
				return
			}
			if acct.Account == nil || acct.Account.Type != "chatgpt" {
				s.err = fmt.Errorf("Codex did not authenticate a ChatGPT account")
				return
			}
			s.models, _ = client.ModelList(s.ctx, 50)
			s.status = CodexStatus{
				BinInstalled: true, BinPath: bin, Authenticated: true,
				LoginMethod: codexLoginMethod(acct.Account.Type, false),
				Email:       acct.Account.Email, Plan: codex.FormatPlan(acct.Account.PlanType),
			}
			return
		}
	}
}
