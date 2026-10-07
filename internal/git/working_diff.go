package git

import (
	"bytes"
	"context"
	"fmt"
	"html"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"

	"recur/internal/domain"
)

// MergeBase returns the merge-base of baseBranch and HEAD in dir. Falls back to
// baseBranch when merge-base fails (e.g. unrelated histories / missing ref).
func MergeBase(ctx context.Context, dir, baseBranch string) string {
	baseBranch = strings.TrimSpace(baseBranch)
	if dir == "" || baseBranch == "" {
		return baseBranch
	}
	out, err := Command(ctx, "-C", dir, "merge-base", "--end-of-options", baseBranch, "HEAD").Output()
	if err != nil {
		return baseBranch
	}
	if mb := strings.TrimSpace(string(out)); mb != "" {
		return mb
	}
	return baseBranch
}

// ChangeStatus returns path → m|a|d|u for changes since baseBranch (the
// merge-base with HEAD), including the working tree. When baseBranch is empty,
// falls back to porcelain status versus HEAD (uncommitted only).
//
// This is the loop-worktree notion of "Changed": everything that differs from
// the point the worktree was spun off the base branch.
func ChangeStatus(ctx context.Context, dir, baseBranch string) map[string]string {
	baseBranch = strings.TrimSpace(baseBranch)
	if dir == "" {
		return nil
	}
	if baseBranch == "" {
		return StatusMap(ctx, dir)
	}
	return changeStatusFlights.do(ctx, dir+"\x00"+baseBranch, func(cctx context.Context) map[string]string {
		return changeStatusWork(cctx, dir, baseBranch)
	})
}

func changeStatusWork(ctx context.Context, dir, baseBranch string) map[string]string {
	if statusComputeHook != nil {
		statusComputeHook("vs-base")
	}
	mb := MergeBase(ctx, dir, baseBranch)
	out, err := Command(ctx, "-C", dir, "diff", "--name-status", "--no-renames", "--end-of-options", mb).Output()
	if err != nil {
		// Fall back to porcelain so the UI still shows something.
		return StatusMap(ctx, dir)
	}
	m := parseNameStatus(string(out))
	// Untracked files never appear in diff-vs-base; fold them in.
	for p, st := range StatusMap(ctx, dir) {
		if st == "u" {
			if _, ok := m[p]; !ok {
				m[p] = "u"
			}
		}
	}
	if len(m) == 0 {
		return nil
	}
	return m
}

func parseNameStatus(out string) map[string]string {
	out = strings.TrimSpace(out)
	if out == "" {
		return map[string]string{}
	}
	m := make(map[string]string)
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		fields := strings.SplitN(line, "\t", 2)
		if len(fields) < 2 {
			continue
		}
		code, path := fields[0], fields[1]
		path = strings.TrimSuffix(filepath.ToSlash(path), "/")
		if path == "" {
			continue
		}
		switch code[0] {
		case 'A':
			m[path] = "a"
		case 'D':
			m[path] = "d"
		case 'M', 'T', 'U':
			m[path] = "m"
		case 'C':
			m[path] = "a"
		default:
			m[path] = "m"
		}
	}
	return m
}

// WorkingDiff returns the diff of path versus baseBranch's merge-base with HEAD
// (working tree included). When baseBranch is empty, diffs versus HEAD.
// Untracked files are rendered as a full-file addition.
//
// Parsing uses the plain (no chroma) path then highlights at most
// maxHighlightLines — large full-file adds stay responsive.
func WorkingDiff(ctx context.Context, dir, baseBranch, rel string) (domain.FileDiff, error) {
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	if dir == "" || rel == "" {
		return domain.FileDiff{}, fmt.Errorf("working diff: dir and path required")
	}

	against := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		against = MergeBase(ctx, dir, b)
	}

	out, err := Command(ctx, "-C", dir, "diff", "--no-color", "--end-of-options", against, "--", rel).Output()
	if err == nil && strings.TrimSpace(string(out)) != "" {
		files := ParseUnifiedDiffPlain(string(out))
		if len(files) > 0 {
			HighlightFiles(files[:1])
			return files[0], nil
		}
	}

	// Cheap single-path porcelain — avoid a full ChangeStatus scan.
	st := pathPorcelain(ctx, dir, rel)
	if st == "" {
		// Clean vs against, or not a git worktree. Do not run `diff --cached`:
		// outside a repo git implies --no-index and rejects --cached
		// ("unknown option `cached'").
		return domain.FileDiff{}, fmt.Errorf("no diff for %s against %s", rel, against)
	}
	// Staged-only changes can be invisible to working-tree diff vs against.
	if st != "u" && st != "a" && st != "?" {
		if cached, cerr := Command(ctx, "-C", dir, "diff", "--no-color", "--cached", "--end-of-options", against, "--", rel).Output(); cerr == nil {
			if files := ParseUnifiedDiffPlain(string(cached)); len(files) > 0 {
				HighlightFiles(files[:1])
				return files[0], nil
			}
		}
	}

	// Untracked/new file: git has no diff for it, so render the whole thing as
	// an addition — bounded, because this path is reached for any file the agent
	// just created, including a multi-megabyte fixture or generated blob.
	abs := filepath.Join(dir, filepath.FromSlash(rel))
	body, truncated, rerr := readCapped(abs, maxSyntheticAddBytes)
	if rerr != nil {
		return domain.FileDiff{}, fmt.Errorf("read %s: %w", rel, rerr)
	}
	fd := syntheticAddDiff(rel, body, truncated)
	outFiles := []domain.FileDiff{fd}
	HighlightFiles(outFiles)
	return outFiles[0], nil
}

// pathPorcelain returns the UI status letter for a single path, or "".
func pathPorcelain(ctx context.Context, dir, rel string) string {
	out, err := Command(ctx, "-C", dir, "status", "--porcelain", "-uall", "--", rel).Output()
	if err != nil {
		return ""
	}
	m := ParseStatusPorcelain(string(out))
	if s := m[rel]; s != "" {
		return s
	}
	// Untracked sometimes reported as directory prefix; treat any hit as untracked.
	for _, s := range m {
		return s
	}
	return ""
}

// Caps for the untracked/new-file synthetic diff. A DiffLine (plus its escaped
// text and stringified line number) is allocated per line and shipped to the
// frontend, so both the bytes read and the lines rendered are bounded — the
// same spirit as maxHighlightLines above and the code package's read caps.
const (
	maxSyntheticAddBytes = 512 * 1024
	maxSyntheticAddLines = 4000
)

// readCapped reads at most limit bytes of path, reporting whether the file was
// longer than that.
func readCapped(path string, limit int64) (content string, truncated bool, err error) {
	f, err := os.Open(path)
	if err != nil {
		return "", false, err
	}
	defer f.Close() //nolint:errcheck
	// Read one extra byte so a file exactly at the limit isn't called truncated.
	buf, err := io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil {
		return "", false, err
	}
	if int64(len(buf)) > limit {
		return string(buf[:limit]), true, nil
	}
	return string(buf), false, nil
}

func omittedUntrackedDiff(path string) domain.FileDiff {
	return domain.FileDiff{
		Path:   path,
		Status: "u",
		Lines: []domain.DiffLine{{
			Type:     "hunk",
			CodeHTML: "diff omitted — too many untracked files",
		}},
	}
}

func contentLooksBinary(content string) bool {
	if content == "" {
		return false
	}
	b := []byte(content)
	return bytes.IndexByte(b, 0) >= 0 || !utf8.Valid(b)
}

func syntheticAddDiff(path, content string, truncated bool) domain.FileDiff {
	if contentLooksBinary(content) {
		return domain.FileDiff{Path: path, Status: "a"}
	}
	f := domain.FileDiff{Path: path, Status: "a"}
	lines := strings.Split(strings.ReplaceAll(content, "\r\n", "\n"), "\n")
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	if len(lines) > maxSyntheticAddLines {
		lines = lines[:maxSyntheticAddLines]
		truncated = true
	}
	f.Lines = append(f.Lines, domain.DiffLine{
		Type: "hunk", CodeHTML: fmt.Sprintf("@@ -0,0 +1,%d @@", len(lines)),
	})
	for i, line := range lines {
		f.Lines = append(f.Lines, domain.DiffLine{
			New:      strconv.Itoa(i + 1),
			Type:     "add",
			CodeHTML: html.EscapeString(line),
		})
		f.Diff.Add++
	}
	if truncated {
		f.Lines = append(f.Lines, domain.DiffLine{
			Type:     "hunk",
			CodeHTML: fmt.Sprintf("@@ truncated — showing the first %d lines of a new file @@", len(lines)),
		})
	}
	return f
}

func revPathSpec(dir, rev, rel string) (spec string, err error) {
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	rev = strings.TrimSpace(rev)
	if dir == "" || rev == "" || rel == "" {
		return "", fmt.Errorf("git rev path: dir, rev, and path required")
	}
	return rev + ":" + rel, nil
}

// FileSizeAtRev returns the object size of path at rev without reading the blob.
// ok=false when the path does not exist at that revision.
func FileSizeAtRev(ctx context.Context, dir, rev, rel string) (size int64, ok bool, err error) {
	spec, err := revPathSpec(dir, rev, rel)
	if err != nil {
		return 0, false, err
	}
	out, err := Command(ctx, "-C", dir, "cat-file", "-s", "--end-of-options", spec).Output()
	if err != nil {
		return 0, false, nil
	}
	n, err := strconv.ParseInt(strings.TrimSpace(string(out)), 10, 64)
	if err != nil {
		return 0, false, fmt.Errorf("file size: %w", err)
	}
	if n < 0 {
		return 0, false, fmt.Errorf("file size: negative")
	}
	return n, true, nil
}

// CopyFileAtRev streams path at rev into dst, stopping after limit bytes.
// ok=false when the path does not exist at that revision.
func CopyFileAtRev(ctx context.Context, dir, rev, rel string, dst io.Writer, limit int64) (ok bool, err error) {
	spec, err := revPathSpec(dir, rev, rel)
	if err != nil {
		return false, err
	}
	if dst == nil {
		return false, fmt.Errorf("copy file: dest required")
	}
	if limit < 0 {
		limit = 0
	}
	cmd := Command(ctx, "-C", dir, "show", "--end-of-options", spec)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return false, err
	}
	cmd.Stderr = io.Discard
	if err := cmd.Start(); err != nil {
		return false, err
	}
	n, copyErr := io.Copy(dst, io.LimitReader(stdout, limit+1))
	if n > limit {
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
		}
		_ = cmd.Wait()
		return false, fmt.Errorf("file too large (%d bytes)", n)
	}
	waitErr := cmd.Wait()
	if waitErr != nil {
		return false, nil
	}
	if copyErr != nil {
		return false, copyErr
	}
	return true, nil
}

// ShowFile returns the contents of path at rev (e.g. a merge-base hash).
// ok=false when the path does not exist at that revision (added/untracked).
func ShowFile(ctx context.Context, dir, rev, rel string) (content string, ok bool, err error) {
	rel = strings.TrimPrefix(filepath.ToSlash(rel), "/")
	rev = strings.TrimSpace(rev)
	if dir == "" || rev == "" || rel == "" {
		return "", false, fmt.Errorf("show file: dir, rev, and path required")
	}
	spec := rev + ":" + rel
	out, err := Command(ctx, "-C", dir, "show", "--end-of-options", spec).Output()
	if err != nil {
		return "", false, nil
	}
	return string(out), true, nil
}

// FileAtBase returns path contents at the merge-base of baseBranch and HEAD
// (or HEAD when baseBranch is empty). Empty string when the file did not exist
// at that revision.
func FileAtBase(ctx context.Context, dir, baseBranch, rel string) (string, error) {
	against := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		against = MergeBase(ctx, dir, b)
	}
	content, ok, err := ShowFile(ctx, dir, against, rel)
	if err != nil {
		return "", err
	}
	if !ok {
		return "", nil
	}
	return content, nil
}
