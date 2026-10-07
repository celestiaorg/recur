package usage

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const cursorAuthFileName = "cursor-usage-auth.json"

type cursorTok struct {
	Access            string
	Refresh           string
	Email             string
	APIKey            string
	APIKeyExpiresAtMs int64
	APIKeyID          int64
	Source            string
}

func cursorAuthPath(dataDir string) string {
	return filepath.Join(dataDir, cursorAuthFileName)
}

// HasRecurCursorAuthFile reports Recur-owned Connect tokens on disk (stat only).
func HasRecurCursorAuthFile(dataDir string) bool {
	if dataDir == "" {
		return false
	}
	_, err := os.Stat(cursorAuthPath(dataDir))
	return err == nil
}

func defaultCursorToken(dataDir string) (cursorTok, error) {
	if dataDir == "" {
		return cursorTok{}, fmt.Errorf("no Cursor usage session")
	}
	tok, err := readCursorAuthFile(cursorAuthPath(dataDir))
	if err != nil || tok.Access == "" {
		return cursorTok{}, fmt.Errorf("no Cursor usage session")
	}
	tok.Source = SourceCursorConnect
	return tok, nil
}

func readCursorAuthFile(path string) (cursorTok, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return cursorTok{}, err
	}
	return parseCursorAuthFile(raw)
}

func parseCursorAuthFile(raw []byte) (cursorTok, error) {
	var file struct {
		AccessToken       string `json:"accessToken"`
		RefreshToken      string `json:"refreshToken"`
		Email             string `json:"email"`
		APIKey            string `json:"apiKey"`
		APIKeyExpiresAtMs int64  `json:"apiKeyExpiresAtMs"`
		APIKeyID          int64  `json:"apiKeyId"`
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		return cursorTok{}, fmt.Errorf("cursor usage auth: %w", err)
	}
	tok := cursorTok{
		Access:            strings.TrimSpace(file.AccessToken),
		Refresh:           strings.TrimSpace(file.RefreshToken),
		Email:             strings.TrimSpace(file.Email),
		APIKey:            strings.TrimSpace(file.APIKey),
		APIKeyExpiresAtMs: file.APIKeyExpiresAtMs,
		APIKeyID:          file.APIKeyID,
		Source:            SourceCursorConnect,
	}
	if tok.Access == "" {
		return cursorTok{}, fmt.Errorf("cursor usage auth is empty")
	}
	return tok, nil
}

func writeCursorAuthFile(path string, tok cursorTok) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	raw, err := json.MarshalIndent(struct {
		AccessToken       string `json:"accessToken"`
		RefreshToken      string `json:"refreshToken"`
		Email             string `json:"email"`
		APIKey            string `json:"apiKey,omitempty"`
		APIKeyExpiresAtMs int64  `json:"apiKeyExpiresAtMs,omitempty"`
		APIKeyID          int64  `json:"apiKeyId,omitempty"`
	}{tok.Access, tok.Refresh, tok.Email, tok.APIKey, tok.APIKeyExpiresAtMs, tok.APIKeyID}, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, raw, 0o600)
}

// RecurCursorAPIKey is the user API key minted from Recur Connect, if any.
func RecurCursorAPIKey(dataDir string) string {
	if dataDir == "" {
		return ""
	}
	tok, err := readCursorAuthFile(cursorAuthPath(dataDir))
	if err != nil {
		return ""
	}
	return tok.APIKey
}

func removeCursorAuthFile(path string) error {
	err := os.Remove(path)
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

// DeleteRecurCursorAuth removes Recur-owned Connect tokens. Callers that can
// revoke the minted dashboard key should do that first (the poller does).
func DeleteRecurCursorAuth(dataDir string) error {
	if dataDir == "" {
		return nil
	}
	return removeCursorAuthFile(cursorAuthPath(dataDir))
}
