package app

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os/exec"
	"path"
	"strings"
	"sync"
	"time"
)

const (
	ghImageMaxBytes = 8 << 20
	ghImageTimeout  = 20 * time.Second
	ghTokenTTL      = 30 * time.Second
)

// ghImageTransport is the RoundTripper used to fetch GitHub attachment bytes.
// Tests replace it.
var ghImageTransport http.RoundTripper = http.DefaultTransport

var ghAuthTokenFn = fetchGHAuthToken

var (
	ghTokenMu sync.Mutex
	ghToken   string
	ghTokenAt time.Time
)

// ServeGitHubImage streams a GitHub-hosted comment/description image into the
// webview at /gh-image?url=. User-attachments 404 without a token, and the
// webview has no GitHub cookies, so this sends `gh auth token` when available.
func ServeGitHubImage(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	raw := strings.TrimSpace(r.URL.Query().Get("url"))
	if raw == "" {
		http.Error(w, "missing url", http.StatusBadRequest)
		return
	}
	target, err := url.Parse(raw)
	if err != nil || !allowedGitHubImageURL(target) {
		http.Error(w, "url not allowed", http.StatusBadRequest)
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), ghImageTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target.String(), nil)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	if token, _ := cachedGHAuthToken(); token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	req.Header.Set("User-Agent", "Recur")
	req.Header.Set("Accept", "image/*,*/*;q=0.8")

	res, err := gitHubImageClient().Do(req)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	defer res.Body.Close() //nolint:errcheck
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		http.Error(w, "upstream "+res.Status, statusFromUpstream(res.StatusCode))
		return
	}
	ct := res.Header.Get("Content-Type")
	if i := strings.IndexByte(ct, ';'); i >= 0 {
		ct = strings.TrimSpace(ct[:i])
	}
	if ct != "" && !strings.HasPrefix(strings.ToLower(ct), "image/") {
		http.Error(w, "not an image", http.StatusUnsupportedMediaType)
		return
	}
	body, err := io.ReadAll(io.LimitReader(res.Body, ghImageMaxBytes+1))
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	if len(body) > ghImageMaxBytes {
		http.Error(w, "image too large", http.StatusRequestEntityTooLarge)
		return
	}
	if ct == "" {
		ct = http.DetectContentType(body)
		if !strings.HasPrefix(ct, "image/") {
			http.Error(w, "not an image", http.StatusUnsupportedMediaType)
			return
		}
	}
	w.Header().Set("Content-Type", ct)
	w.Header().Set("Cache-Control", "private, max-age=86400")
	if r.Method == http.MethodHead {
		w.WriteHeader(http.StatusOK)
		return
	}
	_, _ = w.Write(body)
}

func gitHubImageClient() *http.Client {
	return &http.Client{
		Timeout:   ghImageTimeout,
		Transport: ghImageTransport,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			if len(via) >= 5 {
				return fmt.Errorf("too many redirects")
			}
			if !allowedGitHubImageRedirect(req.URL) {
				return fmt.Errorf("redirect not allowed")
			}
			return nil
		},
	}
}

func cachedGHAuthToken() (string, error) {
	ghTokenMu.Lock()
	if ghToken != "" && time.Since(ghTokenAt) < ghTokenTTL {
		tok := ghToken
		ghTokenMu.Unlock()
		return tok, nil
	}
	ghTokenMu.Unlock()
	tok, err := ghAuthTokenFn()
	if err != nil {
		return "", err
	}
	tok = strings.TrimSpace(tok)
	ghTokenMu.Lock()
	ghToken = tok
	ghTokenAt = time.Now()
	ghTokenMu.Unlock()
	return tok, nil
}

func fetchGHAuthToken() (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "gh", "auth", "token").Output()
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

func resetGHAuthTokenCache() {
	ghTokenMu.Lock()
	ghToken = ""
	ghTokenAt = time.Time{}
	ghTokenMu.Unlock()
}

func allowedGitHubImageURL(u *url.URL) bool {
	if u == nil {
		return false
	}
	if u.Scheme != "https" {
		return false
	}
	if u.User != nil {
		return false
	}
	host := strings.ToLower(u.Hostname())
	if host == "githubusercontent.com" || strings.HasSuffix(host, ".githubusercontent.com") {
		return true
	}
	if host != "github.com" && host != "www.github.com" {
		return false
	}
	p := path.Clean("/" + strings.TrimPrefix(u.Path, "/"))
	if p == "/user-attachments" || strings.HasPrefix(p, "/user-attachments/") {
		return true
	}
	parts := strings.Split(strings.TrimPrefix(p, "/"), "/")
	return len(parts) >= 3 && parts[2] == "assets"
}

// GitHub 302s user-attachments to S3. Allow those hosts only as redirects so
// /gh-image cannot be used as an open S3 proxy.
func allowedGitHubImageRedirect(u *url.URL) bool {
	if allowedGitHubImageURL(u) {
		return true
	}
	if u == nil || u.Scheme != "https" || u.User != nil {
		return false
	}
	host := strings.ToLower(u.Hostname())
	if !strings.HasSuffix(host, ".amazonaws.com") || !strings.Contains(host, ".s3.") {
		return false
	}
	return strings.HasPrefix(host, "github-production-user-asset-") || host == "github-cloud.s3.amazonaws.com"
}

func statusFromUpstream(code int) int {
	if code == http.StatusUnauthorized || code == http.StatusForbidden || code == http.StatusNotFound {
		return code
	}
	return http.StatusBadGateway
}
