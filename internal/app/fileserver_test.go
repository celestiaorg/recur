package app

import (
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"recur/internal/code"
	"recur/internal/domain"
	"recur/internal/engine"
	"recur/internal/store"
)

func TestServeWorkspaceFileStreamsBody(t *testing.T) {
	root := t.TempDir()
	body := strings.Repeat("hello\n", 50_000) // ~300 KiB, streamed rather than inlined
	if err := os.WriteFile(filepath.Join(root, "big.txt"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	png := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
		0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
		0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
		0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
		0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
	}
	if err := os.WriteFile(filepath.Join(root, "icon.png"), png, 0o644); err != nil {
		t.Fatal(err)
	}
	pdf := []byte("%PDF-1.4\n")
	if err := os.WriteFile(filepath.Join(root, "doc.pdf"), pdf, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "blob.bin"), []byte{0x00, 0x01}, 0o644); err != nil {
		t.Fatal(err)
	}

	st := store.NewInMemory()
	p, err := st.AddProject(domain.Project{ID: "p-fs", Name: "FS", RepoPath: root, BaseBranch: "main"})
	if err != nil {
		t.Fatal(err)
	}
	svc := NewServices(engine.New(st))

	t.Run("text", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=big.txt", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %q", res.StatusCode, got)
		}
		if string(got) != body {
			t.Fatalf("got %d bytes, want %d", len(got), len(body))
		}
		if ct := res.Header.Get("Content-Type"); !strings.HasPrefix(ct, "text/plain") {
			t.Fatalf("Content-Type = %q", ct)
		}
	})

	t.Run("image", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=icon.png", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %q", res.StatusCode, got)
		}
		if string(got) != string(png) {
			t.Fatalf("png bytes mismatch (%d vs %d)", len(got), len(png))
		}
		if ct := res.Header.Get("Content-Type"); ct != "image/png" {
			t.Fatalf("Content-Type = %q", ct)
		}
	})

	t.Run("pdf", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=doc.pdf", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("ServeWorkspaceFile(doc.pdf) status = %d body %q, want 200", res.StatusCode, got)
		}
		if string(got) != string(pdf) {
			t.Fatalf("ServeWorkspaceFile(doc.pdf) body len = %d, want %d", len(got), len(pdf))
		}
		if ct := res.Header.Get("Content-Type"); ct != "application/pdf" {
			t.Fatalf("ServeWorkspaceFile(doc.pdf) Content-Type = %q, want application/pdf", ct)
		}
	})

	t.Run("binary rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=blob.bin", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("status %d, want 415", rec.Code)
		}
	})

	t.Run("traversal", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=../secret", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusBadRequest && rec.Code != http.StatusNotFound {
			t.Fatalf("status %d, want 400/404", rec.Code)
		}
	})

	t.Run("loose abs path", func(t *testing.T) {
		loose := filepath.Join(t.TempDir(), "shot.png")
		if err := os.WriteFile(loose, png, 0o644); err != nil {
			t.Fatal(err)
		}
		svc.Code.loose.allow(loose)
		rec := httptest.NewRecorder()
		q := url.Values{"project": {p.ID}, "path": {loose}}
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?"+q.Encode(), nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("loose status %d body %q", res.StatusCode, got)
		}
		if string(got) != string(png) {
			t.Fatalf("loose png bytes mismatch (%d vs %d)", len(got), len(png))
		}
	})

	t.Run("missing params", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
	})
}

func TestServeWorkspaceFileAtBase(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	png := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
		0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
		0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
		0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
		0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
	}
	newer := append([]byte{}, png...)
	newer[len(newer)-1] ^= 0xff
	root := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root}, args...)...)
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
			"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	if err := os.WriteFile(filepath.Join(root, "icon.png"), png, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "notes.txt"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "icon.png"), newer, 0o644); err != nil {
		t.Fatal(err)
	}

	st := store.NewInMemory()
	p, err := st.AddProject(domain.Project{ID: "p-img", Name: "IMG", RepoPath: root, BaseBranch: "main"})
	if err != nil {
		t.Fatal(err)
	}
	svc := NewServices(engine.New(st))

	t.Run("previous revision", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=icon.png&at=base", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %q", res.StatusCode, got)
		}
		if string(got) != string(png) {
			t.Fatalf("at=base served working-tree bytes")
		}
		if ct := res.Header.Get("Content-Type"); ct != "image/png" {
			t.Fatalf("Content-Type = %q", ct)
		}
	})

	t.Run("working tree is still current", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=icon.png", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		got, _ := io.ReadAll(rec.Result().Body)
		if string(got) != string(newer) {
			t.Fatalf("working-tree body mismatch")
		}
	})

	t.Run("text rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=notes.txt&at=base", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
	})

	t.Run("invalid at", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=icon.png&at=HEAD", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("status %d, want 400", rec.Code)
		}
	})

	t.Run("missing at base", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=nope.png&at=base", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusNotFound {
			t.Fatalf("status %d, want 404", rec.Code)
		}
	})

	t.Run("traversal at base", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/workspace-file?project="+p.ID+"&path=../secret.png&at=base", nil)
		ServeWorkspaceFile(svc.Code, rec, req)
		if rec.Code != http.StatusBadRequest && rec.Code != http.StatusNotFound {
			t.Fatalf("status %d, want 400/404", rec.Code)
		}
	})
}

func TestServeChatImage(t *testing.T) {
	t.Cleanup(code.ResetExternalPaths)
	dir := t.TempDir()
	png := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
		0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
		0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
		0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
		0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
	}
	pngPath := filepath.Join(dir, "shot.png")
	if err := os.WriteFile(pngPath, png, 0o644); err != nil {
		t.Fatal(err)
	}
	code.AllowExternalPath(pngPath)
	txtPath := filepath.Join(dir, "notes.txt")
	if err := os.WriteFile(txtPath, []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	code.AllowExternalPath(txtPath)
	missing := filepath.Join(dir, "nope.png")
	code.AllowExternalPath(missing)

	t.Run("png", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/chat-image?path="+pngPath, nil)
		ServeChatImage(rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("ServeChatImage(%q) status = %d body %q, want 200", pngPath, res.StatusCode, got)
		}
		if string(got) != string(png) {
			t.Fatalf("ServeChatImage(%q) body len = %d, want %d", pngPath, len(got), len(png))
		}
		if ct := res.Header.Get("Content-Type"); ct != "image/png" {
			t.Fatalf("ServeChatImage(%q) Content-Type = %q, want image/png", pngPath, ct)
		}
	})

	t.Run("relative rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/chat-image?path=shot.png", nil)
		ServeChatImage(rec, req)
		if rec.Code != http.StatusBadRequest && rec.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("ServeChatImage(relative) status = %d, want 400/415", rec.Code)
		}
	})

	t.Run("txt rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/chat-image?path="+txtPath, nil)
		ServeChatImage(rec, req)
		if rec.Code != http.StatusUnsupportedMediaType && rec.Code != http.StatusBadRequest {
			t.Fatalf("ServeChatImage(txt) status = %d, want 415/400", rec.Code)
		}
	})

	t.Run("missing", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/chat-image?path="+missing, nil)
		ServeChatImage(rec, req)
		if rec.Code != http.StatusNotFound {
			t.Fatalf("ServeChatImage(missing) status = %d, want 404", rec.Code)
		}
	})

	t.Run("method", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodPost, "/chat-image?path="+pngPath, nil)
		ServeChatImage(rec, req)
		if rec.Code != http.StatusMethodNotAllowed {
			t.Fatalf("ServeChatImage(POST) status = %d, want 405", rec.Code)
		}
	})
}

func TestServeExternalFile(t *testing.T) {
	t.Cleanup(code.ResetExternalPaths)
	dir := t.TempDir()
	csvPath := filepath.Join(dir, "export-111.csv")
	body := []byte("a,b\n1,2\n")
	if err := os.WriteFile(csvPath, body, 0o644); err != nil {
		t.Fatal(err)
	}
	binPath := filepath.Join(dir, "blob.bin")
	if err := os.WriteFile(binPath, []byte{0x00, 0x01}, 0o644); err != nil {
		t.Fatal(err)
	}
	code.AllowExternalPath(csvPath)
	code.AllowExternalPath(binPath)

	t.Run("csv", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/external-file?path="+csvPath, nil)
		ServeExternalFile(rec, req)
		res := rec.Result()
		got, _ := io.ReadAll(res.Body)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("ServeExternalFile(%q) status = %d body %q, want 200", csvPath, res.StatusCode, got)
		}
		if string(got) != string(body) {
			t.Fatalf("ServeExternalFile(%q) body = %q, want %q", csvPath, got, body)
		}
		if ct := res.Header.Get("Content-Type"); !strings.HasPrefix(ct, "text/plain") {
			t.Fatalf("ServeExternalFile(%q) Content-Type = %q, want text/plain", csvPath, ct)
		}
	})

	t.Run("binary rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/external-file?path="+binPath, nil)
		ServeExternalFile(rec, req)
		if rec.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("ServeExternalFile(bin) status = %d, want 415", rec.Code)
		}
	})

	t.Run("relative rejected", func(t *testing.T) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/external-file?path=export-111.csv", nil)
		ServeExternalFile(rec, req)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("ServeExternalFile(relative) status = %d, want 400", rec.Code)
		}
	})

	t.Run("undropped rejected", func(t *testing.T) {
		other := filepath.Join(dir, "other.csv")
		if err := os.WriteFile(other, body, 0o644); err != nil {
			t.Fatal(err)
		}
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/external-file?path="+other, nil)
		ServeExternalFile(rec, req)
		if rec.Code != http.StatusForbidden {
			t.Fatalf("ServeExternalFile(undropped) status = %d, want 403", rec.Code)
		}
	})
}
