package code

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
	gitpkg "recur/internal/git"
)

const (
	maxReadBytes      = 1 << 30   // 1 GiB text preview/edit cap
	maxImageBytes     = 512 << 20 // 512 MiB image preview cap
	maxInlineBytes    = 256 << 10 // 256 KiB — larger bodies skip the Wails JSON bridge
	maxEditBytes      = 16 << 20  // 16 MiB — above this, view-only (save stays off the bridge)
	textProbeBytes    = 8 << 10
	maxChatImageBytes = 8 << 20 // chat paste / Finder-drop attachment cap
)

// imageMIME maps lowercase extensions to MIME types shown as binary image
// previews (no text editing). SVG is intentionally omitted — it's editable
// source with a separate frontend preview mode.
var imageMIME = map[string]string{
	".png":  "image/png",
	".jpg":  "image/jpeg",
	".jpeg": "image/jpeg",
	".gif":  "image/gif",
	".webp": "image/webp",
	".ico":  "image/x-icon",
	".bmp":  "image/bmp",
	".avif": "image/avif",
}

// chatImageMIME is the paste / Finder-drop set (matches engine.chatImageMIME).
// Narrower than imageMIME: ico/bmp/avif preview in the observatory but cannot
// be pasted into chat.
var chatImageMIME = map[string]string{
	".png":  "image/png",
	".jpg":  "image/jpeg",
	".jpeg": "image/jpeg",
	".gif":  "image/gif",
	".webp": "image/webp",
}

var pngMagic = []byte{0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a}

// ImageMIME returns the preview MIME type for a path, or "" if not a known image.
func ImageMIME(path string) string {
	return imageMIME[strings.ToLower(filepath.Ext(path))]
}

// PreviewMIME is ImageMIME plus application/pdf for .pdf. Used when serving
// file bytes into the observatory preview — not for raster-only ops
// (ImageAtBase, Outline skip, chat images).
func PreviewMIME(path string) string {
	if mime := ImageMIME(path); mime != "" {
		return mime
	}
	if strings.ToLower(filepath.Ext(path)) == ".pdf" {
		return "application/pdf"
	}
	return ""
}

// ImageAtBase streams an image at the merge-base of baseBranch and HEAD
// (or HEAD when baseBranch is empty) into dst. Used by /workspace-file?at=base
// so the observatory can show a previous/current comparison without pulling
// binary through the Wails JSON bridge. Empty / missing at that revision is
// ErrNotFound. dst may be nil to only stat (HEAD requests).
func ImageAtBase(ctx context.Context, root, baseBranch, rel string, dst io.Writer) error {
	abs, err := SafeJoin(root, rel)
	if err != nil {
		return err
	}
	rel = RelSlash(root, abs)
	if ImageMIME(rel) == "" {
		return fmt.Errorf("not an image")
	}
	against := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		against = gitpkg.MergeBase(ctx, root, b)
	}
	size, ok, err := gitpkg.FileSizeAtRev(ctx, root, against, rel)
	if err != nil {
		return err
	}
	if !ok || size == 0 {
		return notFoundError(rel)
	}
	if size > maxImageBytes {
		return fmt.Errorf("file too large (%d bytes)", size)
	}
	if dst == nil {
		return nil
	}
	ok, err = gitpkg.CopyFileAtRev(ctx, root, against, rel, dst, maxImageBytes)
	if err != nil {
		return err
	}
	if !ok {
		return notFoundError(rel)
	}
	return nil
}

// PrepareChatImage validates an absolute filesystem path for a chat image
// attachment: regular file, accepted extension, size cap, and magic bytes.
func PrepareChatImage(abs string) (clean string, mime string, err error) {
	abs = strings.TrimSpace(abs)
	if abs == "" {
		return "", "", fmt.Errorf("missing path")
	}
	if strings.ContainsRune(abs, 0) {
		return "", "", fmt.Errorf("invalid path")
	}
	clean = filepath.Clean(abs)
	if !filepath.IsAbs(clean) {
		return "", "", fmt.Errorf("path must be absolute")
	}
	if !externalPathAllowed(clean) {
		return "", "", fmt.Errorf("path not allowed")
	}
	mime = chatImageMIME[strings.ToLower(filepath.Ext(clean))]
	if mime == "" {
		return "", "", fmt.Errorf("not a chat image")
	}
	st, err := os.Stat(clean)
	if err != nil {
		if os.IsNotExist(err) {
			return "", "", notFoundError(clean)
		}
		return "", "", err
	}
	if st.IsDir() {
		return "", "", fmt.Errorf("%s is a directory", filepath.Base(clean))
	}
	if !st.Mode().IsRegular() {
		return "", "", fmt.Errorf("not a regular file")
	}
	if st.Size() > maxChatImageBytes {
		return "", "", fmt.Errorf("file too large (%d bytes)", st.Size())
	}
	f, err := os.Open(clean)
	if err != nil {
		if os.IsNotExist(err) {
			return "", "", notFoundError(clean)
		}
		return "", "", err
	}
	defer f.Close() //nolint:errcheck
	buf := make([]byte, 12)
	n, err := io.ReadFull(f, buf)
	if err != nil && err != io.EOF && err != io.ErrUnexpectedEOF {
		return "", "", err
	}
	if !chatImageMagicOK(buf[:n], mime) {
		return "", "", fmt.Errorf("not a chat image")
	}
	return clean, mime, nil
}

func chatImageMagicOK(b []byte, mime string) bool {
	switch mime {
	case "image/png":
		return len(b) >= 8 && bytes.Equal(b[:8], pngMagic)
	case "image/jpeg":
		return len(b) >= 3 && b[0] == 0xff && b[1] == 0xd8 && b[2] == 0xff
	case "image/gif":
		return bytes.HasPrefix(b, []byte("GIF87a")) || bytes.HasPrefix(b, []byte("GIF89a"))
	case "image/webp":
		return len(b) >= 12 && bytes.Equal(b[:4], []byte("RIFF")) && bytes.Equal(b[8:12], []byte("WEBP"))
	default:
		return false
	}
}

// ReadFile loads metadata for a file under root. Text smaller than maxInlineBytes
// is returned in Content so a typical source file is one Wails round-trip.
// Larger text and binary previews leave Content empty — the webview loads the bytes
// from the asset-server /workspace-file route, which streams and does not JSON-
// parse a multi-megabyte payload on the UI thread.
func ReadFile(root, rel string, writable bool) (domain.FileContent, error) {
	full, mime, st, err := statFile(root, rel)
	if err != nil {
		return domain.FileContent{}, err
	}
	out := domain.FileContent{
		Path:     filepath.ToSlash(rel),
		Size:     int(st.Size()),
		Writable: writable,
		Mime:     mime,
		Mtime:    st.ModTime().UnixMilli(),
	}
	if mime != "" {
		return out, nil
	}
	if err := rejectBinaryFile(full, st.Size()); err != nil {
		return domain.FileContent{}, err
	}
	out.Language = languageFor(rel)
	if st.Size() > maxInlineBytes {
		return out, nil
	}
	raw, err := os.ReadFile(full)
	if err != nil {
		if os.IsNotExist(err) {
			return domain.FileContent{}, notFoundError(rel)
		}
		return domain.FileContent{}, err
	}
	if !utf8.Valid(raw) {
		return domain.FileContent{}, fmt.Errorf("binary or non-UTF-8 file")
	}
	out.Content = string(raw)
	out.Size = len(raw)
	return out, nil
}

// PrepareServe resolves a file the asset middleware can stream into the webview.
// It applies the same sandbox, size, and image/text rules as ReadFile, but does
// not load the body — the caller must open Path and copy it out.
func PrepareServe(root, rel string) (abs string, mime string, err error) {
	abs, mime, _, err = statFile(root, rel)
	return abs, mime, err
}

// PrepareExternal validates an absolute filesystem path for the observatory
// editor (Finder-dropped files outside the worktree). Same size / image / binary
// rules as PrepareServe, but no root sandbox — the UI already has the path.
func PrepareExternal(abs string) (clean string, mime string, err error) {
	abs = strings.TrimSpace(abs)
	if abs == "" {
		return "", "", fmt.Errorf("missing path")
	}
	if strings.ContainsRune(abs, 0) {
		return "", "", fmt.Errorf("invalid path")
	}
	clean = filepath.Clean(abs)
	if !filepath.IsAbs(clean) {
		return "", "", fmt.Errorf("path must be absolute")
	}
	if !externalPathAllowed(clean) {
		return "", "", fmt.Errorf("path not allowed")
	}
	st, err := os.Stat(clean)
	if err != nil {
		if os.IsNotExist(err) {
			return "", "", notFoundError(clean)
		}
		return "", "", err
	}
	if st.IsDir() {
		return "", "", fmt.Errorf("%s is a directory", filepath.Base(clean))
	}
	if !st.Mode().IsRegular() {
		return "", "", fmt.Errorf("not a regular file")
	}
	mime = PreviewMIME(clean)
	limit := int64(maxReadBytes)
	if mime != "" {
		limit = maxImageBytes
	}
	if st.Size() > limit {
		return "", "", fmt.Errorf("file too large (%d bytes)", st.Size())
	}
	return clean, mime, nil
}

// RewindBinaryCheck peeks at f and seeks back to the start. Non-image files
// that look binary are rejected with the same error ReadFile uses.
func RewindBinaryCheck(f *os.File) error {
	probe := make([]byte, textProbeBytes)
	n, err := io.ReadFull(f, probe)
	truncated := true
	if err == io.EOF || err == io.ErrUnexpectedEOF {
		truncated = false
		err = nil
	}
	if err != nil {
		return err
	}
	if isBinary(probe[:n], truncated) {
		return fmt.Errorf("binary or non-UTF-8 file")
	}
	_, err = f.Seek(0, io.SeekStart)
	return err
}

func statFile(root, rel string) (abs string, mime string, st os.FileInfo, err error) {
	abs, err = SafeJoin(root, rel)
	if err != nil {
		return "", "", nil, err
	}
	st, err = os.Stat(abs)
	if err != nil {
		if os.IsNotExist(err) {
			return "", "", nil, notFoundError(rel)
		}
		return "", "", nil, err
	}
	if st.IsDir() {
		return "", "", nil, fmt.Errorf("%s is a directory", rel)
	}
	mime = PreviewMIME(rel)
	limit := int64(maxReadBytes)
	if mime != "" {
		limit = maxImageBytes
	}
	if st.Size() > limit {
		return "", "", nil, fmt.Errorf("file too large (%d bytes)", st.Size())
	}
	return abs, mime, st, nil
}

func rejectBinaryFile(full string, size int64) error {
	n := int(size)
	if n > textProbeBytes {
		n = textProbeBytes
	}
	if n <= 0 {
		return nil
	}
	f, err := os.Open(full)
	if err != nil {
		if os.IsNotExist(err) {
			return err
		}
		return err
	}
	defer f.Close() //nolint:errcheck
	probe := make([]byte, n)
	got, err := io.ReadFull(f, probe)
	if err != nil && err != io.EOF && err != io.ErrUnexpectedEOF {
		return err
	}
	if isBinary(probe[:got], size > int64(got)) {
		return fmt.Errorf("binary or non-UTF-8 file")
	}
	return nil
}

func isBinary(b []byte, truncated bool) bool {
	if bytes.IndexByte(b, 0) >= 0 {
		return true
	}
	if truncated {
		b = trimIncompleteUTF8(b)
	}
	return !utf8.Valid(b)
}

// trimIncompleteUTF8 drops a trailing partial rune so a size-capped probe
// does not fail utf8.Valid when byte N falls mid-character.
func trimIncompleteUTF8(b []byte) []byte {
	if utf8.Valid(b) {
		return b
	}
	for i := 1; i < utf8.UTFMax && i <= len(b); i++ {
		if utf8.Valid(b[:len(b)-i]) {
			return b[:len(b)-i]
		}
	}
	return b
}

// WriteFile overwrites a file under root (edit-when-needed). Creates parent dirs.
//
// The write is atomic: this is the editor's save path, and a crash (or a full
// disk) part-way through a plain os.WriteFile would leave the user's source
// truncated with no recovery. Instead we write a sibling temp file, fsync it,
// and rename over the target — rename within a directory is atomic on POSIX,
// so a concurrent reader (or the coding agent) sees either the old file or the
// complete new one, never a half-written one.
func WriteFile(root, rel, content string) error {
	if len(content) > maxEditBytes {
		return fmt.Errorf("file too large to save (%d bytes)", len(content))
	}
	full, err := SafeJoin(root, rel)
	if err != nil {
		return err
	}
	dir := filepath.Dir(full)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	// Preserve the existing mode (e.g. an executable script); 0644 for new files.
	perm := os.FileMode(0o644)
	if st, serr := os.Stat(full); serr == nil {
		perm = st.Mode().Perm()
	}
	tmp, err := os.CreateTemp(dir, "."+filepath.Base(full)+".tmp*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	// No-op once the rename below succeeds; cleans up on every failure path.
	defer os.Remove(tmpName) //nolint:errcheck
	if _, err := tmp.WriteString(content); err != nil {
		tmp.Close() //nolint:errcheck
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close() //nolint:errcheck
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Chmod(tmpName, perm); err != nil {
		return err
	}
	return os.Rename(tmpName, full)
}

// languageFor names the file's language for the frontend editor. It goes
// through git.LexerName so the single cached chroma lexer lookup is shared
// with diff highlighting instead of re-matching every registered lexer.
func languageFor(path string) string {
	return gitpkg.LexerName(filepath.Base(path))
}
