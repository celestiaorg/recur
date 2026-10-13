package app

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/png"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"golang.org/x/image/tiff"

	"recur/internal/code"
)

// transcodeTIFF is true where the webview cannot decode image/tiff
// (WebKitGTK, WebView2). WKWebView renders it natively.
var transcodeTIFF = runtime.GOOS != "darwin"

// maxTIFFPixels caps decoded size so a small compressed TIFF cannot balloon
// into gigabytes of RGBA.
const maxTIFFPixels = 1 << 26

// writeTIFFAsPNG serves a TIFF as PNG for preview. The file is not modified.
func writeTIFFAsPNG(w http.ResponseWriter, src io.Reader) {
	img, err := decodeTIFF(src)
	if err != nil {
		http.Error(w, err.Error(), http.StatusUnsupportedMediaType)
		return
	}
	w.Header().Set("Content-Type", "image/png")
	w.Header().Set("Cache-Control", "no-cache")
	_ = png.Encode(w, img)
}

func decodeTIFF(src io.Reader) (image.Image, error) {
	data, err := io.ReadAll(src)
	if err != nil {
		return nil, err
	}
	cfg, err := tiff.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	if cfg.Width > maxTIFFPixels/cfg.Height {
		return nil, fmt.Errorf("image too large (%dx%d)", cfg.Width, cfg.Height)
	}
	return tiff.Decode(bytes.NewReader(data))
}

// ServeWorkspaceFile streams a project/worktree file into the webview asset
// server at /workspace-file?project=&workspace=&path=. Bodies stay off the
// Wails JSON bridge so opening a large file cannot freeze the UI thread.
//
// Not a bound service method — Wails would otherwise try to generate an RPC
// from http.ResponseWriter. main.go registers this as asset middleware.
func ServeWorkspaceFile(s *CodeService, w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	q := r.URL.Query()
	projectID := q.Get("project")
	rel := q.Get("path")
	if projectID == "" || strings.TrimSpace(rel) == "" {
		http.Error(w, "missing project or path", http.StatusBadRequest)
		return
	}
	if at := strings.TrimSpace(q.Get("at")); at != "" {
		if at != "base" {
			http.Error(w, "invalid at", http.StatusBadRequest)
			return
		}
		serveWorkspaceImageAtBase(s, w, r, projectID, q.Get("workspace"), rel)
		return
	}
	root, rel, err := s.resolveReadPath(projectID, q.Get("workspace"), rel)
	if err != nil {
		status := http.StatusNotFound
		if strings.Contains(err.Error(), "outside the workspace") {
			status = http.StatusForbidden
		}
		http.Error(w, err.Error(), status)
		return
	}
	abs, mime, err := code.PrepareServe(root, rel)
	if err != nil {
		status := http.StatusBadRequest
		if errors.Is(err, code.ErrNotFound) {
			status = http.StatusNotFound
		} else if strings.Contains(err.Error(), "too large") {
			status = http.StatusRequestEntityTooLarge
		}
		http.Error(w, err.Error(), status)
		return
	}
	f, err := os.Open(abs)
	if err != nil {
		if os.IsNotExist(err) {
			http.NotFound(w, r)
			return
		}
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer f.Close() //nolint:errcheck
	if mime == "image/tiff" && transcodeTIFF {
		writeTIFFAsPNG(w, f)
		return
	}
	if mime == "" {
		if err := code.RewindBinaryCheck(f); err != nil {
			http.Error(w, err.Error(), http.StatusUnsupportedMediaType)
			return
		}
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	} else {
		w.Header().Set("Content-Type", mime)
	}
	w.Header().Set("Cache-Control", "no-cache")
	st, err := f.Stat()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.ServeContent(w, r, filepath.Base(abs), st.ModTime(), f)
}

// serveWorkspaceImageAtBase streams the merge-base (or HEAD) revision of a
// raster image. Deleted-on-disk files still exist in git, so this path must
// not go through PrepareServe.
func serveWorkspaceImageAtBase(s *CodeService, w http.ResponseWriter, r *http.Request, projectID, workspaceID, rel string) {
	if filepath.IsAbs(rel) {
		http.Error(w, "base revision is only for workspace paths", http.StatusBadRequest)
		return
	}
	root, _, base, err := s.resolveScope(projectID, workspaceID)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	abs, err := code.SafeJoin(root, rel)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	rel = code.RelSlash(root, abs)
	mime := code.ImageMIME(rel)
	if mime == "" {
		http.Error(w, "not an image", http.StatusBadRequest)
		return
	}
	cw := &countingWriter{ResponseWriter: w}
	w.Header().Set("Content-Type", mime)
	w.Header().Set("Cache-Control", "no-cache")
	dst := io.Writer(cw)
	var tiffBuf *bytes.Buffer
	if r.Method == http.MethodHead {
		dst = nil
	} else if mime == "image/tiff" && transcodeTIFF {
		tiffBuf = new(bytes.Buffer)
		dst = tiffBuf
	}
	if err := code.ImageAtBase(r.Context(), root, base, rel, dst); err != nil {
		status := http.StatusBadRequest
		if errors.Is(err, code.ErrNotFound) {
			status = http.StatusNotFound
		} else if strings.Contains(err.Error(), "too large") {
			status = http.StatusRequestEntityTooLarge
		} else if strings.Contains(err.Error(), "path escapes root") {
			status = http.StatusBadRequest
		}
		if cw.n == 0 {
			http.Error(w, err.Error(), status)
		}
		return
	}
	if tiffBuf != nil {
		writeTIFFAsPNG(w, tiffBuf)
		return
	}
	if r.Method == http.MethodHead {
		w.WriteHeader(http.StatusOK)
	}
}

// countingWriter tracks whether the handler has already flushed a body, so a
// mid-stream git error cannot rewrite the status via http.Error.
type countingWriter struct {
	http.ResponseWriter
	n int
}

func (w *countingWriter) Write(p []byte) (int, error) {
	n, err := w.ResponseWriter.Write(p)
	w.n += n
	return n, err
}

// ServeChatImage streams an absolute-path image into the webview at
// /chat-image?path=. Wails Finder drops only deliver paths (the HTML5 FileList
// is empty), so the composer fetches bytes here and builds the same paste-style
// attachment draft. GET/HEAD only; png/jpeg/gif/webp; 8 MiB cap; magic sniff.
func ServeChatImage(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	abs, mime, err := code.PrepareChatImage(r.URL.Query().Get("path"))
	if err != nil {
		status := http.StatusBadRequest
		if errors.Is(err, code.ErrNotFound) {
			status = http.StatusNotFound
		} else if strings.Contains(err.Error(), "too large") {
			status = http.StatusRequestEntityTooLarge
		} else if strings.Contains(err.Error(), "not a chat image") {
			status = http.StatusUnsupportedMediaType
		} else if strings.Contains(err.Error(), "path not allowed") {
			status = http.StatusForbidden
		}
		http.Error(w, err.Error(), status)
		return
	}
	f, err := os.Open(abs)
	if err != nil {
		if os.IsNotExist(err) {
			http.NotFound(w, r)
			return
		}
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer f.Close() //nolint:errcheck
	w.Header().Set("Content-Type", mime)
	w.Header().Set("Cache-Control", "no-cache")
	st, err := f.Stat()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.ServeContent(w, r, filepath.Base(abs), st.ModTime(), f)
}

// ServeExternalFile streams an absolute-path file into the webview at
// /external-file?path=. Used when the editor opens a Finder-dropped file that
// lives outside the worktree (ReadFile is sandboxed to the project root).
func ServeExternalFile(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	abs, mime, err := code.PrepareExternal(r.URL.Query().Get("path"))
	if err != nil {
		status := http.StatusBadRequest
		if errors.Is(err, code.ErrNotFound) {
			status = http.StatusNotFound
		} else if strings.Contains(err.Error(), "too large") {
			status = http.StatusRequestEntityTooLarge
		} else if strings.Contains(err.Error(), "path not allowed") {
			status = http.StatusForbidden
		}
		http.Error(w, err.Error(), status)
		return
	}
	f, err := os.Open(abs)
	if err != nil {
		if os.IsNotExist(err) {
			http.NotFound(w, r)
			return
		}
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer f.Close() //nolint:errcheck
	if mime == "image/tiff" && transcodeTIFF {
		writeTIFFAsPNG(w, f)
		return
	}
	if mime == "" {
		if err := code.RewindBinaryCheck(f); err != nil {
			http.Error(w, err.Error(), http.StatusUnsupportedMediaType)
			return
		}
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	} else {
		w.Header().Set("Content-Type", mime)
	}
	w.Header().Set("Cache-Control", "no-cache")
	st, err := f.Stat()
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.ServeContent(w, r, filepath.Base(abs), st.ModTime(), f)
}
