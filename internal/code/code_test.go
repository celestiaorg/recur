package code

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"recur/internal/domain"
)

func TestSafeJoinRejectsEscape(t *testing.T) {
	root := t.TempDir()
	if _, err := SafeJoin(root, "../outside"); err == nil {
		t.Fatal("expected escape to fail")
	}
	if _, err := SafeJoin(root, "ok/file.go"); err != nil {
		t.Fatal(err)
	}
}

func TestTreeOutlineSearchSymbols(t *testing.T) {
	root := t.TempDir()
	src := `package demo

func Hello() {}

type Widget struct{}
`
	if err := os.WriteFile(filepath.Join(root, "demo.go"), []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	_ = os.Mkdir(filepath.Join(root, "pkg"), 0o755)
	_ = os.WriteFile(filepath.Join(root, "pkg", "x.go"), []byte("package pkg\nfunc X() {}\n"), 0o644)
	_ = os.WriteFile(filepath.Join(root, ".env"), []byte("X=1\n"), 0o644)
	_ = os.Mkdir(filepath.Join(root, ".github"), 0o755)

	tree, err := ListTree(t.Context(), root, "", 2, "")
	if err != nil || len(tree) < 1 {
		t.Fatalf("tree: %v %#v", err, tree)
	}
	treeNames := map[string]bool{}
	for _, e := range tree {
		treeNames[e.Name] = true
	}
	if !treeNames[".env"] || !treeNames[".github"] {
		t.Fatalf("tree should include hidden entries: %#v", tree)
	}

	out, err := Outline(root, "demo.go")
	if err != nil {
		t.Fatal(err)
	}
	names := map[string]bool{}
	for _, s := range out {
		names[s.Name] = true
	}
	if !names["Hello"] || !names["Widget"] {
		t.Fatalf("outline missing symbols: %#v", out)
	}

	hits, err := Search(t.Context(), root, "Hello")
	if err != nil || len(hits) == 0 {
		t.Fatalf("search: %v %#v", err, hits)
	}

	defs, err := FindDefs(t.Context(), root, "Hello")
	if err != nil || len(defs) == 0 {
		t.Fatalf("defs: %v %#v", err, defs)
	}
	refs, err := FindRefs(t.Context(), root, "Hello")
	if err != nil || len(refs) == 0 {
		t.Fatalf("refs: %v %#v", err, refs)
	}

	fc, err := ReadFile(root, "demo.go", true)
	if err != nil || fc.Content == "" || fc.Language != "go" {
		t.Fatalf("read: %v %#v", err, fc)
	}
	if err := WriteFile(root, "demo.go", src+"\n"); err != nil {
		t.Fatal(err)
	}
}

func TestOutlineMajorLanguages(t *testing.T) {
	root := t.TempDir()
	files := map[string]string{
		"a.py":    "class Widget:\n    def render(self):\n        pass\n\ndef helper():\n    return 1\n",
		"a.rs":    "pub struct Widget {}\npub fn render() {}\npub enum Kind { A }\npub trait Drawable {}\n",
		"A.java":  "public class Widget {\n  public void render() {}\n}\n",
		"a.kt":    "class Widget\nfun render() {}\n",
		"a.swift": "struct Widget {}\nfunc render() {}\n",
		"a.rb":    "class Widget\n  def render\n  end\nend\nmodule Helpers\nend\n",
		"a.php":   "<?php\nclass Widget {}\nfunction render() {}\n",
		"a.cs":    "public class Widget {\n  public void Render() {}\n}\n",
		"a.cpp":   "class Widget {};\nint render() { return 0; }\n",
		"a.dart":  "class Widget {}\nvoid render() {}\n",
		"a.zig":   "pub fn render() void {}\npub const MAX = 1;\n",
	}
	want := map[string][]string{
		"a.py":    {"Widget", "render", "helper"},
		"a.rs":    {"Widget", "render", "Kind", "Drawable"},
		"A.java":  {"Widget", "render"},
		"a.kt":    {"Widget", "render"},
		"a.swift": {"Widget", "render"},
		"a.rb":    {"Widget", "render", "Helpers"},
		"a.php":   {"Widget", "render"},
		"a.cs":    {"Widget", "Render"},
		"a.cpp":   {"Widget", "render"},
		"a.dart":  {"Widget", "render"},
		"a.zig":   {"render", "MAX"},
	}
	for name, src := range files {
		if err := os.WriteFile(filepath.Join(root, name), []byte(src), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	for name, symbols := range want {
		out, err := Outline(root, name)
		if err != nil {
			t.Fatalf("Outline(%s): %v", name, err)
		}
		got := map[string]bool{}
		for _, s := range out {
			got[s.Name] = true
		}
		for _, sym := range symbols {
			if !got[sym] {
				t.Fatalf("Outline(%s) missing %q; got %#v", name, sym, out)
			}
		}
	}
}

func TestOutlineDirectory(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "vendor-lib")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	out, err := Outline(root, "vendor-lib")
	if err != nil {
		t.Fatalf("Outline(dir): %v", err)
	}
	if len(out) != 0 {
		t.Fatalf("Outline(dir) = %#v, want empty", out)
	}
}

func TestReadFileImages(t *testing.T) {
	root := t.TempDir()
	// Minimal valid 1×1 PNG
	png := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
		0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
		0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
		0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
		0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
	}
	cases := []struct {
		name string
		data []byte
		mime string
	}{
		{"icon.png", png, "image/png"},
		{"photo.JPEG", []byte{0xff, 0xd8, 0xff, 0xd9}, "image/jpeg"},
	}
	for _, tc := range cases {
		if err := os.WriteFile(filepath.Join(root, tc.name), tc.data, 0o644); err != nil {
			t.Fatal(err)
		}
		fc, err := ReadFile(root, tc.name, false)
		if err != nil {
			t.Fatalf("ReadFile(%s): %v", tc.name, err)
		}
		if fc.Mime != tc.mime {
			t.Fatalf("ReadFile(%s).Mime = %q, want %q", tc.name, fc.Mime, tc.mime)
		}
		if fc.Content != "" {
			t.Fatalf("ReadFile(%s) inlined image bytes over the bridge: %d", tc.name, len(fc.Content))
		}
		if fc.Size != len(tc.data) {
			t.Fatalf("ReadFile(%s).Size = %d, want %d", tc.name, fc.Size, len(tc.data))
		}
		if fc.Language != "" {
			t.Fatalf("ReadFile(%s) should not set Language for images: %#v", tc.name, fc)
		}
		out, err := Outline(root, tc.name)
		if err != nil || len(out) != 0 {
			t.Fatalf("Outline(%s) = (%v, %v), want empty", tc.name, out, err)
		}
	}

	// SVG is editable text (preview is a frontend concern), not binary image.
	svg := []byte(`<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>`)
	if err := os.WriteFile(filepath.Join(root, "logo.svg"), svg, 0o644); err != nil {
		t.Fatal(err)
	}
	fc, err := ReadFile(root, "logo.svg", true)
	if err != nil {
		t.Fatalf("ReadFile(logo.svg): %v", err)
	}
	if fc.Mime != "" || fc.Content != string(svg) {
		t.Fatalf("ReadFile(logo.svg) = mime=%q content=%q, want text", fc.Mime, fc.Content)
	}

	// Non-image binary still rejected.
	bin := []byte{0x00, 0x01, 0x02, 0xff}
	if err := os.WriteFile(filepath.Join(root, "blob.bin"), bin, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadFile(root, "blob.bin", false); err == nil {
		t.Fatal("ReadFile(blob.bin) expected binary error, got nil")
	}
}

func TestReadFilePDF(t *testing.T) {
	root := t.TempDir()
	pdf := []byte("%PDF-1.4\n")
	if err := os.WriteFile(filepath.Join(root, "doc.pdf"), pdf, 0o644); err != nil {
		t.Fatal(err)
	}
	fc, err := ReadFile(root, "doc.pdf", false)
	if err != nil {
		t.Fatalf("ReadFile(doc.pdf) error = %v, want nil", err)
	}
	if fc.Mime != "application/pdf" {
		t.Errorf("ReadFile(doc.pdf).Mime = %q, want application/pdf", fc.Mime)
	}
	if fc.Content != "" {
		t.Errorf("ReadFile(doc.pdf).Content len = %d, want empty", len(fc.Content))
	}
	if fc.Size != len(pdf) {
		t.Errorf("ReadFile(doc.pdf).Size = %d, want %d", fc.Size, len(pdf))
	}
	if ImageMIME("doc.pdf") != "" {
		t.Errorf("ImageMIME(doc.pdf) = %q, want empty", ImageMIME("doc.pdf"))
	}

	bin := []byte{0x00, 0x01, 0x02, 0xff}
	if err := os.WriteFile(filepath.Join(root, "blob.bin"), bin, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadFile(root, "blob.bin", false); err == nil {
		t.Fatal("ReadFile(blob.bin) error = nil, want binary error")
	}
}

func TestImageAtBase(t *testing.T) {
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
	run("add", ".")
	run("commit", "-m", "init")
	if err := os.WriteFile(filepath.Join(root, "icon.png"), newer, 0o644); err != nil {
		t.Fatal(err)
	}

	var got bytes.Buffer
	if err := ImageAtBase(context.Background(), root, "", "icon.png", &got); err != nil {
		t.Fatal(err)
	}
	if got.String() != string(png) {
		t.Fatalf("ImageAtBase returned working-tree bytes, want HEAD")
	}

	if err := ImageAtBase(context.Background(), root, "", "logo.svg", io.Discard); err == nil {
		t.Fatal("ImageAtBase(svg) expected not an image")
	}
	if err := ImageAtBase(context.Background(), root, "", "missing.png", io.Discard); !errors.Is(err, ErrNotFound) {
		t.Fatalf("ImageAtBase(missing) err = %v, want ErrNotFound", err)
	}
	if err := ImageAtBase(context.Background(), root, "", "../secret.png", io.Discard); err == nil {
		t.Fatal("ImageAtBase(traversal) expected path escape")
	}
}

func TestPrepareChatImage(t *testing.T) {
	t.Cleanup(ResetExternalPaths)
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
	AllowExternalPath(pngPath)

	got, mime, err := PrepareChatImage(pngPath)
	if err != nil {
		t.Fatalf("PrepareChatImage(%q) error = %v, want nil", pngPath, err)
	}
	if got != pngPath {
		t.Errorf("PrepareChatImage(%q) path = %q, want %q", pngPath, got, pngPath)
	}
	if mime != "image/png" {
		t.Errorf("PrepareChatImage(%q) mime = %q, want image/png", pngPath, mime)
	}

	if _, _, err := PrepareChatImage("shot.png"); err == nil {
		t.Fatal("PrepareChatImage(relative) error = nil, want error")
	}
	if _, _, err := PrepareChatImage(""); err == nil {
		t.Fatal("PrepareChatImage(\"\") error = nil, want error")
	}
	if _, _, err := PrepareChatImage(filepath.Join(dir, "missing.png")); err == nil {
		t.Fatal("PrepareChatImage(missing) error = nil, want error")
	}

	txt := filepath.Join(dir, "notes.txt")
	if err := os.WriteFile(txt, []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := PrepareChatImage(txt); err == nil {
		t.Fatal("PrepareChatImage(txt) error = nil, want error")
	}

	fake := filepath.Join(dir, "fake.png")
	if err := os.WriteFile(fake, []byte("not a png"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := PrepareChatImage(fake); err == nil {
		t.Fatal("PrepareChatImage(fake png) error = nil, want error")
	}

	huge := filepath.Join(dir, "huge.png")
	f, err := os.Create(huge)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(maxChatImageBytes + 1); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	if _, _, err := PrepareChatImage(huge); err == nil {
		t.Fatal("PrepareChatImage(huge) error = nil, want error")
	}

	if _, _, err := PrepareChatImage(dir); err == nil {
		t.Fatal("PrepareChatImage(dir) error = nil, want error")
	}
}

func TestPrepareExternal(t *testing.T) {
	t.Cleanup(ResetExternalPaths)
	dir := t.TempDir()
	csvPath := filepath.Join(dir, "export-111.csv")
	body := []byte("a,b\n1,2\n")
	if err := os.WriteFile(csvPath, body, 0o644); err != nil {
		t.Fatal(err)
	}
	secret := filepath.Join(dir, "secret.csv")
	if err := os.WriteFile(secret, body, 0o644); err != nil {
		t.Fatal(err)
	}
	AllowExternalPath(csvPath)

	got, mime, err := PrepareExternal(csvPath)
	if err != nil {
		t.Fatalf("PrepareExternal(%q) error = %v, want nil", csvPath, err)
	}
	if got != csvPath {
		t.Errorf("PrepareExternal(%q) path = %q, want %q", csvPath, got, csvPath)
	}
	if mime != "" {
		t.Errorf("PrepareExternal(%q) mime = %q, want empty", csvPath, mime)
	}

	pdfPath := filepath.Join(dir, "doc.pdf")
	if err := os.WriteFile(pdfPath, []byte("%PDF-1.4\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	AllowExternalPath(pdfPath)
	got, mime, err = PrepareExternal(pdfPath)
	if err != nil {
		t.Fatalf("PrepareExternal(%q) error = %v, want nil", pdfPath, err)
	}
	if got != pdfPath {
		t.Errorf("PrepareExternal(%q) path = %q, want %q", pdfPath, got, pdfPath)
	}
	if mime != "application/pdf" {
		t.Errorf("PrepareExternal(%q) mime = %q, want application/pdf", pdfPath, mime)
	}

	if _, _, err := PrepareExternal(secret); err == nil || !strings.Contains(err.Error(), "path not allowed") {
		t.Fatalf("PrepareExternal(undropped) error = %v, want path not allowed", err)
	}
	if _, _, err := PrepareExternal("export-111.csv"); err == nil {
		t.Fatal("PrepareExternal(relative) error = nil, want error")
	}
	if _, _, err := PrepareExternal(filepath.Join(dir, "missing.csv")); err == nil {
		t.Fatal("PrepareExternal(missing) error = nil, want error")
	}
	if _, _, err := PrepareExternal(dir); err == nil {
		t.Fatal("PrepareExternal(dir) error = nil, want error")
	}
}

func TestExternalAllowPersistsAcrossReset(t *testing.T) {
	t.Cleanup(func() {
		ResetExternalPaths()
		LoadExternalAllowFile("")
	})
	dir := t.TempDir()
	csvPath := filepath.Join(dir, "export-111.csv")
	if err := os.WriteFile(csvPath, []byte("a,b\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	allowFile := filepath.Join(dir, "external-allow.json")
	LoadExternalAllowFile(allowFile)
	AllowExternalPath(csvPath)
	ResetExternalPaths()
	if _, _, err := PrepareExternal(csvPath); err == nil {
		t.Fatal("PrepareExternal after Reset: want path not allowed")
	}
	LoadExternalAllowFile(allowFile)
	if _, _, err := PrepareExternal(csvPath); err != nil {
		t.Fatalf("PrepareExternal after reload: %v", err)
	}
}

func TestExternalAllowEvictsOldest(t *testing.T) {
	t.Cleanup(func() {
		maxExternalAllowed = maxExternalAllowedDefault
		ResetExternalPaths()
		LoadExternalAllowFile("")
	})
	maxExternalAllowed = 2
	dir := t.TempDir()
	write := func(name string) string {
		t.Helper()
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte("x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		return p
	}
	a, b, c := write("a.csv"), write("b.csv"), write("c.csv")
	allowFile := filepath.Join(dir, "external-allow.json")
	LoadExternalAllowFile(allowFile)
	AllowExternalPath(a)
	AllowExternalPath(b)
	AllowExternalPath(c)
	if _, _, err := PrepareExternal(a); err == nil || !strings.Contains(err.Error(), "path not allowed") {
		t.Fatalf("oldest a still allowed: %v", err)
	}
	if _, _, err := PrepareExternal(b); err != nil {
		t.Fatalf("b: %v", err)
	}
	if _, _, err := PrepareExternal(c); err != nil {
		t.Fatalf("c: %v", err)
	}

	ResetExternalPaths()
	LoadExternalAllowFile(allowFile)
	if _, _, err := PrepareExternal(a); err == nil {
		t.Fatal("persisted allowlist still has evicted a")
	}
	if _, _, err := PrepareExternal(b); err != nil {
		t.Fatalf("reloaded b: %v", err)
	}
	if _, _, err := PrepareExternal(c); err != nil {
		t.Fatalf("reloaded c: %v", err)
	}
}

func TestPrepareExternalRejectsFIFO(t *testing.T) {
	t.Cleanup(ResetExternalPaths)
	dir := t.TempDir()
	fifo := filepath.Join(dir, "pipe.csv")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	AllowExternalPath(fifo)
	if _, _, err := PrepareExternal(fifo); err == nil {
		t.Fatal("PrepareExternal(fifo) error = nil, want error")
	}
}

func TestReadFileSizeLimit(t *testing.T) {
	root := t.TempDir()
	over := filepath.Join(root, "over.txt")
	f, err := os.Create(over)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(maxReadBytes + 1); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadFile(root, "over.txt", false); err == nil {
		t.Fatal("expected too-large error")
	} else if !strings.Contains(err.Error(), "file too large") {
		t.Fatalf("err = %v, want file too large", err)
	}

	img := filepath.Join(root, "huge.png")
	pf, err := os.Create(img)
	if err != nil {
		t.Fatal(err)
	}
	if err := pf.Truncate(maxImageBytes + 1); err != nil {
		t.Fatal(err)
	}
	if err := pf.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadFile(root, "huge.png", false); err == nil {
		t.Fatal("expected image too-large error")
	}
}

func TestReadFileInlinesOnlySmallText(t *testing.T) {
	root := t.TempDir()
	small := strings.Repeat("a", 64)
	if err := os.WriteFile(filepath.Join(root, "small.txt"), []byte(small), 0o644); err != nil {
		t.Fatal(err)
	}
	fc, err := ReadFile(root, "small.txt", true)
	if err != nil {
		t.Fatal(err)
	}
	if fc.Content != small || fc.Writable != true || fc.Mtime == 0 {
		t.Fatalf("small: %#v", fc)
	}

	big := strings.Repeat("b", maxInlineBytes+1)
	if err := os.WriteFile(filepath.Join(root, "big.txt"), []byte(big), 0o644); err != nil {
		t.Fatal(err)
	}
	fc, err = ReadFile(root, "big.txt", false)
	if err != nil {
		t.Fatal(err)
	}
	if fc.Content != "" {
		t.Fatalf("large text was inlined (%d bytes)", len(fc.Content))
	}
	if fc.Size != maxInlineBytes+1 {
		t.Fatalf("size = %d, want %d", fc.Size, maxInlineBytes+1)
	}
	if fc.Language == "" {
		t.Fatal("expected language for streamed text")
	}
}

func TestReadFileAcceptsUTF8SplitAtProbeBoundary(t *testing.T) {
	root := t.TempDir()
	// 8191 ASCII bytes + 世 (3-byte rune starting at offset 8191) + trailing text.
	body := strings.Repeat("a", textProbeBytes-1) + "世" + " trailing"
	if err := os.WriteFile(filepath.Join(root, "cjk.txt"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	fc, err := ReadFile(root, "cjk.txt", false)
	if err != nil {
		t.Fatalf("ReadFile: %v", err)
	}
	if fc.Content != body {
		t.Fatalf("content len=%d, want %d", len(fc.Content), len(body))
	}

	f, err := os.Open(filepath.Join(root, "cjk.txt"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := RewindBinaryCheck(f); err != nil {
		t.Fatalf("RewindBinaryCheck: %v", err)
	}
}

func TestWriteFileRejectsOverEditCap(t *testing.T) {
	root := t.TempDir()
	if err := WriteFile(root, "huge.txt", strings.Repeat("a", maxEditBytes+1)); err == nil {
		t.Fatal("expected too-large save error")
	} else if !strings.Contains(err.Error(), "too large") {
		t.Fatalf("err = %v", err)
	}
}

func TestListTreeAnnotatesGitStatus(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
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
	if err := os.WriteFile(filepath.Join(root, "tracked.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, "pkg"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "pkg", "x.go"), []byte("package pkg\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")

	// Modify, delete, add, untracked.
	if err := os.WriteFile(filepath.Join(root, "tracked.go"), []byte("package t\nfunc X() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(filepath.Join(root, "pkg", "x.go")); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "added.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "added.go")
	if err := os.WriteFile(filepath.Join(root, "scratch.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	tree, err := ListTree(t.Context(), root, "", 1, "main")
	if err != nil {
		t.Fatal(err)
	}
	byPath := map[string]domain.TreeEntry{}
	for _, e := range tree {
		byPath[e.Path] = e
	}
	if byPath["tracked.go"].Status != "m" {
		t.Errorf("tracked.go status=%q want m", byPath["tracked.go"].Status)
	}
	if byPath["added.go"].Status != "a" {
		t.Errorf("added.go status=%q want a", byPath["added.go"].Status)
	}
	if byPath["scratch.go"].Status != "u" {
		t.Errorf("scratch.go status=%q want u", byPath["scratch.go"].Status)
	}

	pkgKids, err := ListTree(t.Context(), root, "pkg", 1, "main")
	if err != nil {
		t.Fatal(err)
	}
	foundDel := false
	for _, e := range pkgKids {
		if e.Path == "pkg/x.go" && e.Status == "d" {
			foundDel = true
		}
	}
	if !foundDel {
		t.Fatalf("expected deleted pkg/x.go in listing: %#v", pkgKids)
	}
}

func TestListTreeAnnotatesIgnored(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
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
	if err := os.WriteFile(filepath.Join(root, ".gitignore"), []byte("scratch.bin\nbuild/\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "tracked.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, ".github"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, ".github", "ci.yml"), []byte("name: ci\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, "build"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "build", "out.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "scratch.bin"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".gitignore", "tracked.go", ".github")
	run("commit", "-m", "init")

	tree, err := ListTree(t.Context(), root, "", 1, "")
	if err != nil {
		t.Fatal(err)
	}
	byPath := map[string]domain.TreeEntry{}
	for _, e := range tree {
		byPath[e.Path] = e
	}
	if !byPath["scratch.bin"].Ignored {
		t.Errorf("ListTree(...)[scratch.bin].Ignored = false, want true")
	}
	if !byPath["build"].Ignored {
		t.Errorf("ListTree(...)[build].Ignored = false, want true")
	}
	if byPath[".gitignore"].Ignored {
		t.Errorf("ListTree(...)[.gitignore].Ignored = true, want false (tracked)")
	}
	if byPath[".github"].Ignored {
		t.Errorf("ListTree(...)[.github].Ignored = true, want false (tracked)")
	}
	if byPath["tracked.go"].Ignored {
		t.Errorf("ListTree(...)[tracked.go].Ignored = true, want false")
	}

	buildKids, err := ListTree(t.Context(), root, "build", 1, "")
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, e := range buildKids {
		if e.Path == "build/out.txt" {
			found = true
			if !e.Ignored {
				t.Errorf("ListTree(build)[build/out.txt].Ignored = false, want true")
			}
		}
	}
	if !found {
		t.Fatalf("ListTree(build) missing build/out.txt: %#v", buildKids)
	}

	batch, err := ListTrees(t.Context(), root, []string{"", "build"}, 1, "")
	if err != nil {
		t.Fatal(err)
	}
	if !entryIgnored(batch[""], "scratch.bin") {
		t.Errorf("ListTrees(...)[\"\"].scratch.bin.Ignored = false, want true")
	}
	if entryIgnored(batch[""], ".gitignore") {
		t.Errorf("ListTrees(...)[\"\"].gitignore.Ignored = true, want false")
	}
	if !entryIgnored(batch["build"], "build/out.txt") {
		t.Errorf("ListTrees(...)[build].out.txt.Ignored = false, want true")
	}
}

func TestListTreeTreatsSymlinkDirsAsDirs(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, "real"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "real", "inside.go"), []byte("package real\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "file.go"), []byte("package t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("real", filepath.Join(root, "link")); err != nil {
		t.Skipf("symlink: %v", err)
	}
	if err := os.Symlink("file.go", filepath.Join(root, "filelink")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("missing", filepath.Join(root, "dangling")); err != nil {
		t.Fatal(err)
	}

	tree, err := ListTree(t.Context(), root, "", 1, "")
	if err != nil {
		t.Fatal(err)
	}
	byPath := map[string]domain.TreeEntry{}
	for _, e := range tree {
		byPath[e.Path] = e
	}
	if !byPath["real"].Dir {
		t.Errorf("ListTree(...)[real].Dir = false, want true")
	}
	if !byPath["link"].Dir {
		t.Errorf("ListTree(...)[link].Dir = false, want true (symlink to dir)")
	}
	if byPath["file.go"].Dir {
		t.Errorf("ListTree(...)[file.go].Dir = true, want false")
	}
	if byPath["filelink"].Dir {
		t.Errorf("ListTree(...)[filelink].Dir = true, want false (symlink to file)")
	}
	if byPath["dangling"].Dir {
		t.Errorf("ListTree(...)[dangling].Dir = true, want false (broken symlink)")
	}

	kids, err := ListTree(t.Context(), root, "link", 1, "")
	if err != nil {
		t.Fatalf("ListTree(..., \"link\", ...) error = %v, want nil", err)
	}
	found := false
	for _, e := range kids {
		if e.Path == "link/inside.go" && !e.Dir {
			found = true
		}
	}
	if !found {
		t.Fatalf("ListTree(..., \"link\", ...) missing link/inside.go: %#v", kids)
	}

	deep, err := ListTree(t.Context(), root, "", 2, "")
	if err != nil {
		t.Fatal(err)
	}
	var linkKids []domain.TreeEntry
	for _, e := range deep {
		if e.Path == "link" {
			linkKids = e.Children
		}
	}
	found = false
	for _, e := range linkKids {
		if e.Path == "link/inside.go" {
			found = true
		}
	}
	if !found {
		t.Fatalf("ListTree(..., \"\", 2, ...)[link] missing child link/inside.go: %#v", linkKids)
	}
}

func entryIgnored(ents []domain.TreeEntry, path string) bool {
	for _, e := range ents {
		if e.Path == path {
			return e.Ignored
		}
	}
	return false
}

func TestResolveRootPrefersWorktree(t *testing.T) {
	repo := t.TempDir()
	wt := t.TempDir()
	root, err := ResolveRoot(repo, wt)
	if err != nil {
		t.Fatal(err)
	}
	if root != wt {
		// ResolveRoot Abs-normalizes; compare via same Abs.
		absWT, _ := filepath.Abs(wt)
		if root != absWT {
			t.Fatalf("got %q want worktree %q", root, absWT)
		}
	}
}

func TestResolveRootInfoMissingWorktreeDoesNotFallBack(t *testing.T) {
	repo := t.TempDir()
	if err := os.WriteFile(filepath.Join(repo, "only-in-repo.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	missing := filepath.Join(repo, "gone-wt")
	root, writable, err := ResolveRootInfo(repo, missing)
	if !errors.Is(err, ErrWorktreeMissing) {
		t.Fatalf("ResolveRootInfo(%q, %q) error = %v, want ErrWorktreeMissing", repo, missing, err)
	}
	if root != "" {
		t.Fatalf("ResolveRootInfo(%q, %q) root = %q, want empty (must not fall back to the repo)", repo, missing, root)
	}
	if writable {
		t.Fatalf("ResolveRootInfo(%q, %q) writable = true, want false", repo, missing)
	}
	const sniff = "worktree is not a directory"
	if ErrWorktreeMissing.Error() != sniff {
		t.Fatalf("ErrWorktreeMissing.Error() = %q, want %q (frontend isWorktreeMissing sniffs this)", ErrWorktreeMissing.Error(), sniff)
	}
	if !strings.Contains(err.Error(), sniff) {
		t.Fatalf("ResolveRootInfo error %q must keep %q for the frontend sniff", err, sniff)
	}
}

// WriteFile saves via temp+rename. Verify the rename actually lands, that the
// existing file mode survives (an executable script must stay executable), and
// that no stray .tmp sibling is left behind.
func TestWriteFileIsAtomicAndPreservesMode(t *testing.T) {
	root := t.TempDir()
	script := filepath.Join(root, "run.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\necho old\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := WriteFile(root, "run.sh", "#!/bin/sh\necho new\n"); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(script)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "#!/bin/sh\necho new\n" {
		t.Fatalf("content = %q", got)
	}
	st, err := os.Stat(script)
	if err != nil {
		t.Fatal(err)
	}
	if st.Mode().Perm() != 0o755 {
		t.Fatalf("mode = %v, want 0755 preserved", st.Mode().Perm())
	}

	// New file in a new dir: created 0644, parents made.
	if err := WriteFile(root, "sub/dir/new.txt", "hello"); err != nil {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(filepath.Join(root, "sub", "dir", "new.txt")); err != nil || string(b) != "hello" {
		t.Fatalf("new file = %q, %v", b, err)
	}

	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if strings.Contains(e.Name(), ".tmp") {
			t.Fatalf("temp file left behind: %s", e.Name())
		}
	}
}
