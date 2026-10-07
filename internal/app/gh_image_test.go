package app

import (
	"bytes"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestAllowedGitHubImageURL(t *testing.T) {
	ok := []string{
		"https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
		"https://www.github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
		"https://github.com/acme/app/assets/1/bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
		"https://private-user-images.githubusercontent.com/1/shot.png?jwt=x",
		"https://user-images.githubusercontent.com/1/shot.png",
	}
	for _, raw := range ok {
		u, err := url.Parse(raw)
		if err != nil {
			t.Fatalf("parse %q: %v", raw, err)
		}
		if !allowedGitHubImageURL(u) {
			t.Errorf("allowedGitHubImageURL(%q) = false, want true", raw)
		}
	}
	bad := []string{
		"https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png",
		"https://evil.example/pixel.png",
		"https://github.com/login",
		"https://github.com/acme/app/blob/main/shot.png",
		"https://github.com.evil.example/user-attachments/assets/x",
		"https://notgithubusercontent.com/shot.png",
		"https://user:pass@github.com/user-attachments/assets/x",
		"https://github.com/user-attachments/../login",
		"http://github.com/user-attachments/assets/x",
		"ftp://github.com/user-attachments/assets/x",
		"/user-attachments/assets/x",
	}
	for _, raw := range bad {
		u, err := url.Parse(raw)
		if err != nil {
			t.Fatalf("parse %q: %v", raw, err)
		}
		if allowedGitHubImageURL(u) {
			t.Errorf("allowedGitHubImageURL(%q) = true, want false", raw)
		}
	}
}

func TestServeGitHubImage(t *testing.T) {
	png := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
		0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
		0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
		0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
		0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
	}
	asset := "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"

	t.Cleanup(func() {
		ghImageTransport = http.DefaultTransport
		ghAuthTokenFn = fetchGHAuthToken
		resetGHAuthTokenCache()
	})
	resetGHAuthTokenCache()
	ghAuthTokenFn = func() (string, error) { return "test-token", nil }

	t.Run("proxies with bearer token", func(t *testing.T) {
		var gotAuth string
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			gotAuth = req.Header.Get("Authorization")
			if req.URL.String() != asset {
				t.Errorf("upstream url = %q, want %q", req.URL, asset)
			}
			return &http.Response{
				StatusCode: http.StatusOK,
				Header:     http.Header{"Content-Type": []string{"image/png"}},
				Body:       io.NopCloser(bytes.NewReader(png)),
				Request:    req,
			}, nil
		})
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(asset), nil)
		ServeGitHubImage(rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %q", res.StatusCode, got)
		}
		if gotAuth != "Bearer test-token" {
			t.Errorf("Authorization = %q", gotAuth)
		}
		if !bytes.Equal(got, png) {
			t.Fatalf("body mismatch (%d vs %d)", len(got), len(png))
		}
		if ct := res.Header.Get("Content-Type"); ct != "image/png" {
			t.Errorf("Content-Type = %q", ct)
		}
	})

	t.Run("rejects off-allowlist url", func(t *testing.T) {
		called := false
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			called = true
			return nil, nil
		})
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape("https://evil.example/pixel.png"), nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
		if called {
			t.Fatal("fetched off-allowlist url")
		}
	})

	t.Run("rejects html upstream", func(t *testing.T) {
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			return &http.Response{
				StatusCode: http.StatusOK,
				Header:     http.Header{"Content-Type": []string{"text/html; charset=utf-8"}},
				Body:       io.NopCloser(strings.NewReader("<html>login</html>")),
				Request:    req,
			}, nil
		})
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(asset), nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("status %d, want 415", rec.Code)
		}
	})

	t.Run("missing url", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image", nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
	})

	t.Run("rejects http url", func(t *testing.T) {
		called := false
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			called = true
			return nil, nil
		})
		httpURL := "http://github.com/user-attachments/assets/x"
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(httpURL), nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("ServeGitHubImage(http url) status = %d, want 400", rec.Code)
		}
		if called {
			t.Fatal("ServeGitHubImage fetched http url")
		}
	})

	t.Run("public image without token", func(t *testing.T) {
		resetGHAuthTokenCache()
		ghAuthTokenFn = func() (string, error) { return "", nil }
		var gotAuth string
		public := "https://user-images.githubusercontent.com/1/shot.png"
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			gotAuth = req.Header.Get("Authorization")
			return &http.Response{
				StatusCode: http.StatusOK,
				Header:     http.Header{"Content-Type": []string{"image/png"}},
				Body:       io.NopCloser(bytes.NewReader(png)),
				Request:    req,
			}, nil
		})
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(public), nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusOK {
			t.Fatalf("ServeGitHubImage(public, no token) status = %d, want 200", rec.Code)
		}
		if gotAuth != "" {
			t.Errorf("Authorization = %q, want empty", gotAuth)
		}
	})

	t.Run("follows GitHub attachment redirect to S3", func(t *testing.T) {
		s3 := "https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png?X-Amz-Signature=x"
		var hosts []string
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			hosts = append(hosts, req.URL.Host)
			if req.URL.Host == "github.com" {
				return &http.Response{
					StatusCode: http.StatusFound,
					Header:     http.Header{"Location": []string{s3}},
					Body:       io.NopCloser(strings.NewReader("")),
					Request:    req,
				}, nil
			}
			if req.Header.Get("Authorization") != "" {
				t.Errorf("Authorization leaked to S3: %q", req.Header.Get("Authorization"))
			}
			return &http.Response{
				StatusCode: http.StatusOK,
				Header:     http.Header{"Content-Type": []string{"image/png"}},
				Body:       io.NopCloser(bytes.NewReader(png)),
				Request:    req,
			}, nil
		})
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(asset), nil)
		ServeGitHubImage(rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %q hosts %v", res.StatusCode, got, hosts)
		}
		if !bytes.Equal(got, png) {
			t.Fatalf("body mismatch (%d vs %d)", len(got), len(png))
		}
		if len(hosts) < 2 || hosts[0] != "github.com" || !strings.Contains(hosts[1], "s3.amazonaws.com") {
			t.Errorf("hosts = %v", hosts)
		}
	})

	t.Run("does not fetch S3 as the initial url", func(t *testing.T) {
		called := false
		ghImageTransport = roundTripFunc(func(req *http.Request) (*http.Response, error) {
			called = true
			return nil, nil
		})
		s3 := "https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png"
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/gh-image?url="+url.QueryEscape(s3), nil)
		ServeGitHubImage(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
		if called {
			t.Fatal("fetched S3 as initial url")
		}
	})

	t.Run("allows redirect to GitHub user-asset S3", func(t *testing.T) {
		assetURL, err := url.Parse(asset)
		if err != nil {
			t.Fatalf("parse asset: %v", err)
		}
		s3, err := url.Parse("https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png")
		if err != nil {
			t.Fatalf("parse s3: %v", err)
		}
		client := gitHubImageClient()
		if err := client.CheckRedirect(&http.Request{URL: s3}, []*http.Request{{URL: assetURL}}); err != nil {
			t.Fatalf("CheckRedirect(S3) = %v, want nil", err)
		}
	})

	t.Run("rejects https to http redirect", func(t *testing.T) {
		assetURL, err := url.Parse(asset)
		if err != nil {
			t.Fatalf("parse asset: %v", err)
		}
		downgrade, err := url.Parse("http://github.com/user-attachments/assets/x")
		if err != nil {
			t.Fatalf("parse downgrade: %v", err)
		}
		client := gitHubImageClient()
		err = client.CheckRedirect(
			&http.Request{URL: downgrade},
			[]*http.Request{{URL: assetURL}},
		)
		if err == nil {
			t.Fatal("CheckRedirect(https→http) = nil, want error")
		}
	})
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
