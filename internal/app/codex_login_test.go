package app

import (
	"context"
	"encoding/json"
	"errors"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/codex"
)

// Re-exec this native test binary as a hermetic Codex app-server.
func TestMain(m *testing.M) {
	if os.Getenv("RECUR_TEST_CODEX_LOGIN") != "" && len(os.Args) > 1 && os.Args[1] == "app-server" {
		fakeCodexLoginServer()
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func fakeCodexLoginServer() {
	dec, enc := json.NewDecoder(os.Stdin), json.NewEncoder(os.Stdout)
	home := os.Getenv("CODEX_HOME")
	for {
		var req struct {
			ID     any             `json:"id"`
			Method string          `json:"method"`
			Params json.RawMessage `json:"params"`
		}
		if dec.Decode(&req) != nil {
			return
		}
		var result any = map[string]any{}
		switch req.Method {
		case "initialized":
			continue
		case "account/login/start":
			var params struct{ Type string }
			_ = json.Unmarshal(req.Params, &params)
			if params.Type != "chatgpt" || os.Getenv("CODEX_API_KEY") != "" || os.Getenv("OPENAI_API_KEY") != "" {
				return
			}
			_ = enc.Encode(map[string]any{"id": req.ID, "result": codex.LoginStartResult{
				LoginID: "codex-login", AuthURL: "https://auth.openai.com/test-login",
			}})
			mode := os.Getenv("RECUR_TEST_CODEX_LOGIN")
			if mode == "exit" {
				return
			}
			if mode == "pending" {
				continue
			}
			if mode == "success" {
				_ = os.WriteFile(filepath.Join(home, "auth.json"), []byte(`{"tokens":{"access_token":"recur-test"}}`), 0o600)
			}
			_ = enc.Encode(map[string]any{"method": "account/login/completed", "params": codex.LoginCompleted{
				LoginID: "codex-login", Success: mode == "success", Error: "Login rejected",
			}})
			continue
		case "account/login/cancel":
			var params struct {
				LoginID string `json:"loginId"`
			}
			_ = json.Unmarshal(req.Params, &params)
			_ = os.WriteFile(filepath.Join(home, "cancelled"), []byte(params.LoginID), 0o600)
		case "account/read":
			result = codex.AccountReadResult{}
			if _, err := os.Stat(filepath.Join(home, "auth.json")); err == nil {
				result = codex.AccountReadResult{Account: &codex.Account{Type: "chatgpt", Email: "recur@example.com", PlanType: "plus"}}
			}
		case "model/list":
			result = map[string]any{"data": []codex.Model{{ID: "gpt-test", DisplayName: "GPT Test"}}}
		}
		_ = enc.Encode(map[string]any{"id": req.ID, "result": result})
	}
}

func codexLoginTestService(t *testing.T, mode string) (*SettingsService, string) {
	t.Helper()
	dataDir := t.TempDir()
	t.Setenv("RECUR_DATA_DIR", dataDir)
	t.Setenv("HOME", t.TempDir())
	t.Setenv("RECUR_TEST_CODEX_LOGIN", mode)
	bin, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv(codex.EnvBinPath, bin)
	svc, _ := newTestServices(t)
	t.Cleanup(func() { _ = svc.Settings.ServiceShutdown() })
	return svc.Settings, dataDir
}

// Opt-in smoke for the actual Codex protocol, requiring no account or browser.
func TestCodexBrowserLoginLive(t *testing.T) {
	if os.Getenv("RECUR_CODEX_LOGIN_E2E") == "" {
		t.Skip("set RECUR_CODEX_LOGIN_E2E=1 and RECUR_CODEX to a native Codex binary")
	}
	t.Setenv("RECUR_DATA_DIR", t.TempDir())
	t.Setenv("HOME", t.TempDir())
	svc, _ := newTestServices(t)
	t.Cleanup(func() { _ = svc.Settings.ServiceShutdown() })
	login, err := svc.Settings.StartCodexLogin()
	if err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(login.AuthURL)
	if err != nil || u.Scheme != "https" || u.Host == "" || login.LoginID == "" {
		t.Fatal("Codex did not return a valid HTTPS browser sign-in URL")
	}
	svc.Settings.CancelCodexLogin(login.LoginID)
}

func TestCodexBrowserLogin(t *testing.T) {
	svc, dataDir := codexLoginTestService(t, "success")
	t.Setenv("CODEX_API_KEY", "ambient-codex-key")
	t.Setenv("OPENAI_API_KEY", "ambient-openai-key")
	set := svc.GetSettings()
	set.CodexAPIKey = "saved-api-key"
	set.CodexChatGPTAuth = false
	mustSetSettings(t, svc, set)
	login, err := svc.StartCodexLogin()
	if err != nil {
		t.Fatal(err)
	}
	if login.AuthURL != "https://auth.openai.com/test-login" || login.LoginID == "" {
		t.Fatalf("login = %+v", login)
	}
	status, err := svc.WaitCodexLogin(login.LoginID)
	if err != nil || !status.Authenticated || status.Email != "recur@example.com" {
		t.Fatalf("status = %+v, err = %v", status, err)
	}
	if set = svc.GetSettings(); !set.CodexChatGPTAuth || set.CodexAPIKey != "" {
		t.Fatal("successful login did not switch from API key to ChatGPT auth")
	}
	if _, err := os.Stat(filepath.Join(dataDir, "codex-home", "auth.json")); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(codex.DefaultSeedAuthPath()); !os.IsNotExist(err) {
		t.Fatalf("browser login touched the machine-wide credentials: %v", err)
	}
	// A new probe process must work with only Recur's saved credentials.
	if status = svc.GetCodexStatus(); !status.Authenticated {
		t.Fatalf("saved login could not be reused: %+v", status)
	}
}

func TestCodexBrowserLoginFailureAndRetry(t *testing.T) {
	svc, _ := codexLoginTestService(t, "failed")
	login, err := svc.StartCodexLogin()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := svc.WaitCodexLogin(login.LoginID); err == nil || !strings.Contains(err.Error(), "Login rejected") {
		t.Fatalf("login failure = %v", err)
	}
	if svc.GetSettings().CodexChatGPTAuth {
		t.Fatal("failed login enabled ChatGPT auth")
	}
	t.Setenv("RECUR_TEST_CODEX_LOGIN", "success")
	login, err = svc.StartCodexLogin()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := svc.WaitCodexLogin(login.LoginID); err != nil {
		t.Fatalf("retry failed: %v", err)
	}
}

func TestCodexBrowserLoginCancelAndShutdown(t *testing.T) {
	svc, dataDir := codexLoginTestService(t, "pending")
	login, err := svc.StartCodexLogin()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := svc.StartCodexLogin(); err == nil {
		t.Fatal("allowed overlapping login attempts")
	}
	svc.CancelCodexLogin("older-attempt")
	svc.CancelCodexLogin(login.LoginID)
	if raw, err := os.ReadFile(filepath.Join(dataDir, "codex-home", "cancelled")); err != nil || string(raw) != "codex-login" {
		t.Fatalf("Codex did not receive login cancellation: %q, %v", raw, err)
	}
	if _, err := svc.WaitCodexLogin(login.LoginID); err == nil {
		t.Fatal("cancelled login was accepted")
	}
	if _, err := svc.StartCodexLogin(); err != nil {
		t.Fatalf("could not restart after cancellation: %v", err)
	}
	if err := svc.ServiceShutdown(); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.StartCodexLogin(); err == nil {
		t.Fatal("started login after shutdown")
	}
	if svc.GetSettings().CodexChatGPTAuth {
		t.Fatal("cancelled login enabled ChatGPT auth")
	}
}

func TestCodexBrowserLoginTimeoutAndExit(t *testing.T) {
	for _, mode := range []string{"pending", "exit"} {
		t.Run(mode, func(t *testing.T) {
			_, dataDir := codexLoginTestService(t, mode)
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			session := &codexLoginSession{
				id: "test", ctx: ctx, cancel: cancel,
				started: make(chan codexLoginStart, 1), done: make(chan struct{}),
			}
			go session.run(os.Getenv(codex.EnvBinPath), dataDir)
			<-session.started
			select {
			case <-session.done:
			case <-time.After(5 * time.Second):
				t.Fatal("login worker did not stop")
			}
			if session.err == nil || errors.Is(session.err, context.Canceled) {
				t.Fatalf("unexpected login result: %v", session.err)
			}
		})
	}
}
