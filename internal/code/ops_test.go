package code

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCreateMkdirRenameCopyRemove(t *testing.T) {
	root := t.TempDir()

	if err := CreateFile(root, "src/hello.go"); err != nil {
		t.Fatalf("CreateFile: %v", err)
	}
	if err := CreateFile(root, "src/hello.go"); err == nil {
		t.Fatal("CreateFile duplicate should fail")
	}
	raw, err := os.ReadFile(filepath.Join(root, "src", "hello.go"))
	if err != nil || len(raw) != 0 {
		t.Fatalf("CreateFile contents: %v %#v", err, raw)
	}

	if err := Mkdir(root, "src/pkg"); err != nil {
		t.Fatalf("Mkdir: %v", err)
	}
	if err := Mkdir(root, "src/pkg"); err != nil {
		t.Fatalf("Mkdir idempotent: %v", err)
	}
	if err := CreateFile(root, "src/pkg/x.go"); err != nil {
		t.Fatalf("CreateFile nested: %v", err)
	}
	if err := os.WriteFile(filepath.Join(root, "src", "pkg", "x.go"), []byte("package pkg\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	if err := Rename(root, "src/hello.go", "src/hi.go"); err != nil {
		t.Fatalf("Rename file: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "src", "hello.go")); !os.IsNotExist(err) {
		t.Fatalf("old name should be gone: %v", err)
	}

	if err := Copy(root, "src/pkg", "src/pkg2"); err != nil {
		t.Fatalf("Copy dir: %v", err)
	}
	got, err := os.ReadFile(filepath.Join(root, "src", "pkg2", "x.go"))
	if err != nil || string(got) != "package pkg\n" {
		t.Fatalf("Copy dir contents: %v %q", err, got)
	}

	if err := Copy(root, "src/hi.go", "src/hi_copy.go"); err != nil {
		t.Fatalf("Copy file: %v", err)
	}
	if err := Copy(root, "src/hi.go", "src/hi_copy.go"); err == nil {
		t.Fatal("Copy duplicate should fail")
	}

	if err := Remove(root, "src/pkg2"); err != nil {
		t.Fatalf("Remove dir: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "src", "pkg2")); !os.IsNotExist(err) {
		t.Fatalf("removed dir should be gone: %v", err)
	}
	if err := Remove(root, "src/hi_copy.go"); err != nil {
		t.Fatalf("Remove file: %v", err)
	}
}

func TestOpsRejectRootAndEscape(t *testing.T) {
	root := t.TempDir()
	for _, fn := range []func() error{
		func() error { return CreateFile(root, "") },
		func() error { return Mkdir(root, ".") },
		func() error { return Remove(root, "") },
		func() error { return Rename(root, "../x", "y") },
		func() error { return Copy(root, "a", "../b") },
	} {
		if err := fn(); err == nil {
			t.Fatal("expected rejection")
		}
	}
}

func TestCopyRenameRefuseSelfNest(t *testing.T) {
	root := t.TempDir()
	if err := Mkdir(root, "a/b"); err != nil {
		t.Fatal(err)
	}
	if err := CreateFile(root, "a/b/f.go"); err != nil {
		t.Fatal(err)
	}
	if err := Copy(root, "a", "a/nested"); err == nil {
		t.Fatal("Copy into self should fail")
	}
	if err := Rename(root, "a", "a/b/moved"); err == nil {
		t.Fatal("Rename into self should fail")
	}
}

func TestAbsPath(t *testing.T) {
	root := t.TempDir()
	got, err := AbsPath(root, "src/x.go")
	if err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(root, "src", "x.go")
	wantAbs, _ := filepath.Abs(want)
	if got != wantAbs {
		t.Fatalf("AbsPath = %q, want %q", got, wantAbs)
	}
}
