package projfile

import (
	"bytes"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strconv"

	toml "github.com/pelletier/go-toml/v2"
	"github.com/pelletier/go-toml/v2/unstable"
	"github.com/pelletier/go-toml/v2/unstable/edit"

	"recur/internal/domain"
)

func writeShape(p domain.Project) File {
	runs := domain.EffectiveRuns(p)
	fileRuns := make([]fileRun, len(runs))
	for i, r := range runs {
		fileRuns[i] = fileRun{Name: r.Name, Cmd: r.Cmd}
	}
	return File{
		SetupCmd:    p.SetupCmd,
		TeardownCmd: p.TeardownCmd,
		AppURL:      p.AppURL,
		Env:         p.Env,
		CopyFiles:   p.CopyFiles,
		Runs:        fileRuns,
	}
}

// Write creates .recur/project.toml from p's run-config. When a parseable
// file already exists, values are patched in place so comments, unknown
// keys, and layout stay. A missing or malformed file is rewritten from
// scratch (with the standard header).
func Write(p domain.Project) error {
	root := repoRoot(p.RepoPath)
	if root == "" {
		return fmt.Errorf("write %s: empty repo path", RelPath)
	}
	st, err := os.Stat(root)
	if err != nil {
		if os.IsNotExist(err) {
			return fmt.Errorf("write %s: repo path %q does not exist", RelPath, root)
		}
		return fmt.Errorf("write %s: stat repo path: %w", RelPath, err)
	}
	if !st.IsDir() {
		return fmt.Errorf("write %s: repo path %q is not a directory", RelPath, root)
	}
	dir := filepath.Join(root, ".recur")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("create %s: %w", dir, err)
	}
	path := filepath.Join(dir, "project.toml")
	data, err := encodeProjectFile(path, p)
	if err != nil {
		return err
	}
	return writeAtomic(path, data)
}

func encodeProjectFile(path string, p domain.Project) ([]byte, error) {
	existing, err := os.ReadFile(path)
	if err != nil {
		if !os.IsNotExist(err) {
			return nil, fmt.Errorf("read %s: %w", RelPath, err)
		}
		return marshalNew(p)
	}
	merged, err := mergeProjectFile(existing, p)
	if err != nil {
		log.Printf("recur: %s: rewrite after parse/merge error: %v", RelPath, err)
		return marshalNew(p)
	}
	return merged, nil
}

func marshalNew(p domain.Project) ([]byte, error) {
	var buf bytes.Buffer
	buf.WriteString(fileHeader)
	enc := toml.NewEncoder(&buf)
	if err := enc.Encode(writeShape(p)); err != nil {
		return nil, fmt.Errorf("marshal %s: %w", RelPath, err)
	}
	return buf.Bytes(), nil
}

func mergeProjectFile(existing []byte, p domain.Project) ([]byte, error) {
	d, err := edit.Parse(existing)
	if err != nil {
		return nil, err
	}
	shape := writeShape(p)
	if err := setString(d, "setup_cmd", shape.SetupCmd); err != nil {
		return nil, err
	}
	if err := setString(d, "teardown_cmd", shape.TeardownCmd); err != nil {
		return nil, err
	}
	if err := setString(d, "app_url", shape.AppURL); err != nil {
		return nil, err
	}
	if err := setString(d, "env", shape.Env); err != nil {
		return nil, err
	}
	if err := setString(d, "copy_files", shape.CopyFiles); err != nil {
		return nil, err
	}
	if err := mergeRuns(d, shape.Runs); err != nil {
		return nil, err
	}
	d.Delete([]string{"dev_cmd"})
	return d.Bytes(), nil
}

func marshalRunTables(runs []fileRun) ([]byte, error) {
	if len(runs) == 0 {
		return nil, nil
	}
	var buf bytes.Buffer
	if err := toml.NewEncoder(&buf).Encode(struct {
		Runs []fileRun `toml:"run"`
	}{Runs: runs}); err != nil {
		return nil, fmt.Errorf("marshal %s [[run]]: %w", RelPath, err)
	}
	return buf.Bytes(), nil
}

func appendSection(doc, extra []byte) []byte {
	if len(extra) == 0 {
		return doc
	}
	out := bytes.TrimRight(doc, "\n")
	out = append(out, '\n', '\n')
	return append(out, extra...)
}

func setString(d *edit.Document, key, value string) error {
	if cur, ok := d.Get([]string{key}); ok {
		if s, ok := cur.(string); ok && s == value {
			return nil
		}
	} else if value == "" {
		return nil
	}
	raw, err := encodeStringValue(value)
	if err != nil {
		return err
	}
	return d.Set([]string{key}, unstable.RawMessage(raw))
}

func encodeStringValue(s string) ([]byte, error) {
	type wrap struct {
		V string `toml:"v" multiline:"true"`
	}
	var buf bytes.Buffer
	if err := toml.NewEncoder(&buf).Encode(wrap{V: s}); err != nil {
		return nil, fmt.Errorf("encode %s string: %w", RelPath, err)
	}
	const prefix = "v = "
	b := buf.Bytes()
	if !bytes.HasPrefix(b, []byte(prefix)) {
		return nil, fmt.Errorf("encode %s string: unexpected %q", RelPath, b)
	}
	return bytes.TrimSpace(b[len(prefix):]), nil
}

func mergeRuns(d *edit.Document, runs []fileRun) error {
	n := existingRunCount(d)
	if len(runs) == 0 {
		if n > 0 {
			d.Delete([]string{"run"})
		}
		return nil
	}
	if n == 0 {
		// edit.Set(["run","0",…]) would create [run.0] tables, which Overlay
		// cannot read as [[run]]. Append real array-of-tables blocks instead.
		blocks, err := marshalRunTables(runs)
		if err != nil {
			return err
		}
		patched, err := edit.Parse(appendSection(d.Bytes(), blocks))
		if err != nil {
			return err
		}
		*d = *patched
		return nil
	}
	for i, r := range runs {
		idx := strconv.Itoa(i)
		if err := setRunField(d, idx, "name", r.Name); err != nil {
			return err
		}
		if err := setRunField(d, idx, "cmd", r.Cmd); err != nil {
			return err
		}
	}
	for i := n - 1; i >= len(runs); i-- {
		d.Delete([]string{"run", strconv.Itoa(i)})
	}
	return nil
}

func existingRunCount(d *edit.Document) int {
	v, ok := d.Get([]string{"run"})
	if !ok {
		return 0
	}
	arr, ok := v.([]any)
	if !ok {
		d.Delete([]string{"run"})
		return 0
	}
	return len(arr)
}

func setRunField(d *edit.Document, idx, field, value string) error {
	path := []string{"run", idx, field}
	if cur, ok := d.Get(path); ok {
		if s, ok := cur.(string); ok && s == value {
			return nil
		}
	}
	if field == "cmd" {
		raw, err := encodeStringValue(value)
		if err != nil {
			return err
		}
		return d.Set(path, unstable.RawMessage(raw))
	}
	return d.Set(path, value)
}

func writeAtomic(path string, data []byte) error {
	invalidate(path)
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".project.toml.*")
	if err != nil {
		return fmt.Errorf("write %s: %w", RelPath, err)
	}
	tmpName := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return fmt.Errorf("write %s: %w", RelPath, err)
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpName)
		return fmt.Errorf("write %s: %w", RelPath, err)
	}
	if err := os.Rename(tmpName, path); err != nil {
		os.Remove(tmpName)
		return fmt.Errorf("write %s: %w", RelPath, err)
	}
	return nil
}
