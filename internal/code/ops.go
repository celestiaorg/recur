package code

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

// CreateFile creates an empty file under root. Parents are created as needed.
// Fails if the path already exists.
func CreateFile(root, rel string) error {
	full, err := safeTarget(root, rel)
	if err != nil {
		return err
	}
	if _, err := os.Lstat(full); err == nil {
		return fmt.Errorf("%s already exists", filepath.ToSlash(rel))
	} else if !os.IsNotExist(err) {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		return err
	}
	f, err := os.OpenFile(full, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	return f.Close()
}

// Mkdir creates a directory under root (and any missing parents).
// Fails if a file already exists at the path; no-op if the directory exists.
func Mkdir(root, rel string) error {
	full, err := safeTarget(root, rel)
	if err != nil {
		return err
	}
	st, err := os.Lstat(full)
	if err == nil {
		if st.IsDir() {
			return nil
		}
		return fmt.Errorf("%s already exists", filepath.ToSlash(rel))
	}
	if !os.IsNotExist(err) {
		return err
	}
	return os.MkdirAll(full, 0o755)
}

// Rename moves a file or directory within root. Destination must not exist.
func Rename(root, fromRel, toRel string) error {
	from, err := safeTarget(root, fromRel)
	if err != nil {
		return err
	}
	to, err := safeTarget(root, toRel)
	if err != nil {
		return err
	}
	if from == to {
		return nil
	}
	if _, err := os.Lstat(from); err != nil {
		return err
	}
	if _, err := os.Lstat(to); err == nil {
		return fmt.Errorf("%s already exists", filepath.ToSlash(toRel))
	} else if !os.IsNotExist(err) {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		return err
	}
	// Refuse moving a directory into itself.
	if isNestedPath(from, to) {
		return fmt.Errorf("cannot move into itself")
	}
	return os.Rename(from, to)
}

// Remove deletes a file or directory tree under root. The repo root itself is refused.
func Remove(root, rel string) error {
	full, err := safeTarget(root, rel)
	if err != nil {
		return err
	}
	if _, err := os.Lstat(full); err != nil {
		return err
	}
	return os.RemoveAll(full)
}

// Copy duplicates a file or directory tree within root. Destination must not exist.
func Copy(root, fromRel, toRel string) error {
	from, err := safeTarget(root, fromRel)
	if err != nil {
		return err
	}
	to, err := safeTarget(root, toRel)
	if err != nil {
		return err
	}
	if from == to {
		return fmt.Errorf("source and destination are the same")
	}
	st, err := os.Lstat(from)
	if err != nil {
		return err
	}
	if _, err := os.Lstat(to); err == nil {
		return fmt.Errorf("%s already exists", filepath.ToSlash(toRel))
	} else if !os.IsNotExist(err) {
		return err
	}
	if st.IsDir() {
		if isNestedPath(from, to) {
			return fmt.Errorf("cannot copy into itself")
		}
		return copyDir(from, to)
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		return err
	}
	return copyFile(from, to, st.Mode())
}

// AbsPath returns the absolute filesystem path for a repo-relative path.
func AbsPath(root, rel string) (string, error) {
	return SafeJoin(root, rel)
}

// safeTarget is SafeJoin that also rejects empty/root targets for mutating ops.
func safeTarget(root, rel string) (string, error) {
	rel = strings.TrimSpace(rel)
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	if rel == "" || rel == "." {
		return "", fmt.Errorf("path required")
	}
	return SafeJoin(root, rel)
}

func isNestedPath(parent, child string) bool {
	prefix := parent + string(os.PathSeparator)
	return strings.HasPrefix(child, prefix)
}

func copyFile(from, to string, mode os.FileMode) error {
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(to, os.O_CREATE|os.O_EXCL|os.O_WRONLY, mode.Perm())
	if err != nil {
		return err
	}
	_, copyErr := io.Copy(out, in)
	closeErr := out.Close()
	if copyErr != nil {
		_ = os.Remove(to)
		return copyErr
	}
	if closeErr != nil {
		_ = os.Remove(to)
		return closeErr
	}
	return nil
}

func copyDir(from, to string) error {
	if err := os.MkdirAll(to, 0o755); err != nil {
		return err
	}
	entries, err := os.ReadDir(from)
	if err != nil {
		return err
	}
	for _, e := range entries {
		src := filepath.Join(from, e.Name())
		dst := filepath.Join(to, e.Name())
		info, err := e.Info()
		if err != nil {
			return err
		}
		if e.IsDir() {
			if err := copyDir(src, dst); err != nil {
				return err
			}
			continue
		}
		if info.Mode()&os.ModeSymlink != 0 {
			continue // skip symlinks — stay inside the sandbox
		}
		if err := copyFile(src, dst, info.Mode()); err != nil {
			return err
		}
	}
	return nil
}
