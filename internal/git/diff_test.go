package git

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"recur/internal/domain"
)

// commit stages all changes in dir and commits them with a fixed identity.
func commit(t *testing.T, dir, msg string) {
	t.Helper()
	for _, args := range [][]string{{"add", "-A"}, {"commit", "-m", msg}} {
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=Coder", "GIT_AUTHOR_EMAIL=coder@recur.local",
			"GIT_COMMITTER_NAME=Coder", "GIT_COMMITTER_EMAIL=coder@recur.local",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
}

func write(t *testing.T, dir, rel, content string) {
	t.Helper()
	p := filepath.Join(dir, rel)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

// TestDiffPRShapes is the M6 git-diff parsing test (brief A): a temp repo with a
// branch making a known change → the PR-wide diff has the right FileDiff/DiffLine
// shapes (add/del/ctx/hunk + line numbers) and stats; commits parse with role +
// iteration; per-commit diff matches.
func TestDiffPRShapes(t *testing.T) {
	repo := initRepo(t) // main with README.md "hello\n"
	ctx := context.Background()

	// Branch off main and make a known change: edit README + add a new file.
	gitInDir(t, repo, "checkout", "-b", "recur/feature")
	write(t, repo, "README.md", "hello world\nsecond line\n")
	write(t, repo, "feature.txt", "alpha\nbeta\n")
	commit(t, repo, "recur(Coder): iteration 1 — add a feature")

	m := NewManager(repo, t.TempDir())

	// ---- PR-wide diff ----
	pr, err := m.DiffPR(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffPR: %v", err)
	}
	if len(pr.Files) != 2 {
		t.Fatalf("expected 2 changed files, got %d: %+v", len(pr.Files), pr.Files)
	}

	files := map[string]int{}
	for i, f := range pr.Files {
		files[f.Path] = i
	}

	readmeIdx, ok := files["README.md"]
	if !ok {
		t.Fatalf("README.md missing from diff: %+v", files)
	}
	readme := pr.Files[readmeIdx]
	if readme.Status != "m" {
		t.Errorf("README.md status: got %q, want m", readme.Status)
	}
	// README had 1 line; new has 2 lines with the first changed → 1 add of the
	// original removed + 2 added (hello world, second line). Assert at least one
	// del and two adds, with a hunk header and correct line types.
	var sawHunk, sawAdd, sawDel bool
	for _, ln := range readme.Lines {
		switch ln.Type {
		case "hunk":
			sawHunk = true
			if !strings.HasPrefix(ln.CodeHTML, "Line") {
				t.Errorf("hunk label should be GitHub-style (Line/Lines …), got %q", ln.CodeHTML)
			}
			if strings.Contains(ln.CodeHTML, "@@") {
				t.Errorf("hunk label must not include raw @@ chrome, got %q", ln.CodeHTML)
			}
		case "add":
			sawAdd = true
			if ln.New == "" {
				t.Errorf("add line should have a new line number, got %+v", ln)
			}
			if ln.Old != "" {
				t.Errorf("add line should have NO old line number, got %+v", ln)
			}
		case "del":
			sawDel = true
			if ln.Old == "" {
				t.Errorf("del line should have an old line number, got %+v", ln)
			}
			if ln.New != "" {
				t.Errorf("del line should have NO new line number, got %+v", ln)
			}
		}
	}
	if !sawHunk || !sawAdd || !sawDel {
		t.Errorf("README diff missing line kinds: hunk=%v add=%v del=%v lines=%+v", sawHunk, sawAdd, sawDel, readme.Lines)
	}

	feat := pr.Files[files["feature.txt"]]
	if feat.Status != "a" {
		t.Errorf("feature.txt status: got %q, want a (added)", feat.Status)
	}
	if feat.Diff.Add != 2 || feat.Diff.Del != 0 {
		t.Errorf("feature.txt stat: got +%d -%d, want +2 -0", feat.Diff.Add, feat.Diff.Del)
	}
	// A new file should have only add + hunk lines, with ascending new line numbers.
	wantNew := []string{"1", "2"}
	var gotNew []string
	for _, ln := range feat.Lines {
		if ln.Type == "add" {
			gotNew = append(gotNew, ln.New)
		}
	}
	if strings.Join(gotNew, ",") != strings.Join(wantNew, ",") {
		t.Errorf("feature.txt add line numbers: got %v, want %v", gotNew, wantNew)
	}

	// PR-wide stat rolls up the files.
	if pr.Add == 0 {
		t.Errorf("PR-wide additions should be > 0, got %d", pr.Add)
	}

	// ---- commit list ----
	commits, err := m.LogCommits(ctx, repo, "main")
	if err != nil {
		t.Fatalf("LogCommits: %v", err)
	}
	if len(commits) != 1 {
		t.Fatalf("expected 1 branch commit, got %d: %+v", len(commits), commits)
	}
	c := commits[0]
	if c.AuthorRole != "Coder" || c.AgentCode != "CO" {
		t.Errorf("commit role/code: got %q/%q, want Coder/CO", c.AuthorRole, c.AgentCode)
	}
	if c.IterLabel != "iteration 1" {
		t.Errorf("commit iter label: got %q, want 'iteration 1'", c.IterLabel)
	}
	if c.Diff.Add == 0 {
		t.Errorf("commit diff stat additions should be > 0, got %+v", c.Diff)
	}

	// ---- per-commit diff ----
	cfiles, err := m.CommitDiff(ctx, repo, c.Hash)
	if err != nil {
		t.Fatalf("CommitDiff: %v", err)
	}
	if len(cfiles) != 2 {
		t.Errorf("per-commit diff should have 2 files, got %d", len(cfiles))
	}
}

// TestDiffWorkingVsBaseIncludesUncommitted is the Observatory files-changed
// view: committed branch work plus dirty/untracked files versus the base.
func TestDiffWorkingVsBaseIncludesUncommitted(t *testing.T) {
	repo := initRepo(t) // main with README.md "hello\n"
	ctx := context.Background()
	gitInDir(t, repo, "checkout", "-b", "recur/feature")
	write(t, repo, "feature.txt", "alpha\n")
	commit(t, repo, "add feature")
	write(t, repo, "feature.txt", "alpha\nbeta\n")
	write(t, repo, "scratch.txt", "untracked\n")

	m := NewManager(repo, t.TempDir())
	pr, err := m.DiffPR(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffPR: %v", err)
	}
	if len(pr.Files) != 1 || pr.Files[0].Path != "feature.txt" {
		t.Fatalf("DiffPR files = %+v, want committed feature.txt only", pr.Files)
	}

	got, err := m.DiffWorkingVsBase(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase: %v", err)
	}
	byPath := map[string]domain.FileDiff{}
	for _, f := range got.Files {
		byPath[f.Path] = f
	}
	if _, ok := byPath["feature.txt"]; !ok {
		t.Errorf("DiffWorkingVsBase missing committed+dirty feature.txt: %+v", byPath)
	}
	scratch, ok := byPath["scratch.txt"]
	if !ok {
		t.Fatalf("DiffWorkingVsBase missing untracked scratch.txt: %+v", byPath)
	}
	if scratch.Status != "a" {
		t.Errorf("scratch.txt status = %q, want a", scratch.Status)
	}
	if scratch.Diff.Add < 1 {
		t.Errorf("scratch.txt add = %d, want ≥ 1", scratch.Diff.Add)
	}

	headOnly, err := m.DiffWorkingVsBase(ctx, repo, "")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase vs HEAD: %v", err)
	}
	headPaths := map[string]bool{}
	for _, f := range headOnly.Files {
		headPaths[f.Path] = true
	}
	if !headPaths["feature.txt"] || !headPaths["scratch.txt"] {
		t.Errorf("DiffWorkingVsBase vs HEAD = %+v, want dirty feature.txt + scratch.txt", headPaths)
	}
}

// TestDiffWorkingVsBaseUnbornHEAD: git init has no HEAD; diff versus the empty
// tree so staged files (and synthetic untracked) still show as additions.
func TestDiffWorkingVsBaseUnbornHEAD(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	dir := t.TempDir()
	gitInDir(t, dir, "init", "-b", "main")
	write(t, dir, "staged.txt", "in index\n")
	gitInDir(t, dir, "add", "staged.txt")
	write(t, dir, "loose.txt", "untracked\n")

	got, err := NewManager(dir, t.TempDir()).DiffWorkingVsBase(context.Background(), dir, "")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase unborn: %v", err)
	}
	byPath := map[string]bool{}
	for _, f := range got.Files {
		byPath[f.Path] = true
	}
	if !byPath["staged.txt"] || !byPath["loose.txt"] {
		t.Fatalf("DiffWorkingVsBase unborn = %+v, want staged.txt + loose.txt", byPath)
	}
}

func TestCommitDiffMergeCommit(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	gitInDir(t, repo, "checkout", "-b", "feature")
	write(t, repo, "side.txt", "from feature\n")
	commit(t, repo, "add side")
	gitInDir(t, repo, "checkout", "main")
	gitInDir(t, repo, "merge", "--no-ff", "-m", "merge feature", "feature")

	m := NewManager(repo, t.TempDir())
	cmd := exec.Command("git", "rev-parse", "HEAD")
	cmd.Dir = repo
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	hash := strings.TrimSpace(string(out))

	plain, err := m.CommitDiff(ctx, repo, hash)
	if err != nil {
		t.Fatalf("CommitDiff(%s): %v", hash, err)
	}
	if len(plain) != 0 {
		t.Errorf("CommitDiff(%s) files = %+v, want empty (no --first-parent)", hash, plain)
	}

	files, err := m.CommitDiffFirstParent(ctx, repo, hash)
	if err != nil {
		t.Fatalf("CommitDiffFirstParent(%s): %v", hash, err)
	}
	found := false
	for _, f := range files {
		if f.Path == "side.txt" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("CommitDiffFirstParent(%s) files = %+v, want side.txt", hash, files)
	}
}

// A "sync with base" merge (`git merge main` on the feature branch) has a first-
// parent diff of everything main advanced by. CommitDiff must stay empty so eager
// fill does not persist that delta; the drill-in path uses FirstParent.
func TestCommitDiffSyncWithBaseMerge(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	gitInDir(t, repo, "checkout", "-b", "feature")
	write(t, repo, "feat.txt", "on feature\n")
	commit(t, repo, "add feat")
	gitInDir(t, repo, "checkout", "main")
	write(t, repo, "m.txt", "from main\n")
	commit(t, repo, "advance main")
	gitInDir(t, repo, "checkout", "feature")
	gitInDir(t, repo, "merge", "--no-ff", "-m", "sync with base", "main")

	m := NewManager(repo, t.TempDir())
	cmd := exec.Command("git", "rev-parse", "HEAD")
	cmd.Dir = repo
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("rev-parse HEAD: %v", err)
	}
	hash := strings.TrimSpace(string(out))

	plain, err := m.CommitDiff(ctx, repo, hash)
	if err != nil {
		t.Fatalf("CommitDiff(%s): %v", hash, err)
	}
	if len(plain) != 0 {
		t.Errorf("CommitDiff(%s) files = %+v, want empty (eager fill must not embed the base delta)", hash, plain)
	}

	files, err := m.CommitDiffFirstParent(ctx, repo, hash)
	if err != nil {
		t.Fatalf("CommitDiffFirstParent(%s): %v", hash, err)
	}
	found := false
	for _, f := range files {
		if f.Path == "m.txt" {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("CommitDiffFirstParent(%s) files = %+v, want m.txt", hash, files)
	}

	commits, err := m.LogCommits(ctx, repo, "main")
	if err != nil {
		t.Fatalf("LogCommits: %v", err)
	}
	if len(commits) != 2 {
		t.Fatalf("LogCommits() len = %d, want 2 (merge + feat)", len(commits))
	}
	if commits[0].Message != "sync with base" {
		t.Errorf("LogCommits()[0].Message = %q, want %q", commits[0].Message, "sync with base")
	}
	if commits[0].Diff.Add < 1 {
		t.Errorf("LogCommits() merge Diff = %+v, want add ≥ 1 (first-parent numstat for m.txt)", commits[0].Diff)
	}
	if commits[1].Message != "add feat" {
		t.Errorf("LogCommits()[1].Message = %q, want %q", commits[1].Message, "add feat")
	}
}

func TestDiffStatVsBaseIncludesWorkingTree(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()

	gitInDir(t, repo, "checkout", "-b", "recur/feature")
	write(t, repo, "feat.txt", "one\ntwo\n")
	commit(t, repo, "recur(Coder): iteration 1 — feat")

	add, del, err := DiffStatVsBase(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffStatVsBase committed: %v", err)
	}
	if add < 2 || del != 0 {
		t.Fatalf("committed stat: got +%d −%d, want ≥+2 −0", add, del)
	}

	// Uncommitted edit should grow the sidebar rollup (working tree vs merge-base).
	write(t, repo, "feat.txt", "one\ntwo\nthree\n")
	add2, del2, err := DiffStatVsBase(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffStatVsBase dirty: %v", err)
	}
	if add2 <= add {
		t.Fatalf("dirty tree should increase additions: before +%d after +%d", add, add2)
	}
	if del2 != 0 {
		t.Fatalf("unexpected deletions: %d", del2)
	}

	// Main working-copy baseline: empty base → vs HEAD (only the uncommitted line).
	addHead, delHead, err := DiffStatVsBase(ctx, repo, "")
	if err != nil {
		t.Fatalf("DiffStatVsBase vs HEAD: %v", err)
	}
	if addHead < 1 || delHead != 0 {
		t.Fatalf("vs HEAD: got +%d −%d, want ≥+1 −0", addHead, delHead)
	}

	// Brand-new untracked file must grow additions before it is staged.
	write(t, repo, "brand-new.txt", "alpha\nbeta\ngamma\n")
	add3, _, err := DiffStatVsBase(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffStatVsBase untracked: %v", err)
	}
	if add3 < add2+3 {
		t.Fatalf("untracked file should add ≥3 lines: before +%d after +%d", add2, add3)
	}
}

func TestUntrackedAddLinesHonorsCancel(t *testing.T) {
	repo := initRepo(t)
	for i := 0; i < 40; i++ {
		write(t, repo, fmt.Sprintf("bulk/f%d.txt", i), "x\n")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	// Already-canceled: must return promptly (partial/zero), not scan unbounded.
	done := make(chan int, 1)
	go func() { done <- untrackedAddLines(ctx, repo) }()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("untrackedAddLines ignored canceled context")
	}
}

// TestParseUnifiedDiffEdgeCases covers an empty diff, a binary file, and HTML
// escaping of code content (no injection).
func TestParseUnifiedDiffHunkContentDashes(t *testing.T) {
	// Regression: inside a hunk, a deleted line whose CONTENT starts with "-- "
	// renders as "--- ..." and an added "++ " line as "+++ ..." — these must be
	// classified as del/add content, NOT swallowed as file headers (which would drop
	// the row and desync line numbers).
	diff := "diff --git a/q.sql b/q.sql\n" +
		"--- a/q.sql\n" +
		"+++ b/q.sql\n" +
		"@@ -1,2 +1,2 @@\n" +
		" SELECT 1;\n" +
		"--- old comment\n" +
		"+++ new thing\n"
	files := ParseUnifiedDiffPlain(diff)
	if len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}
	var gotDel, gotAdd bool
	for _, ln := range files[0].Lines {
		// CodeHTML may wrap tokens in highlight spans — match on content.
		if ln.Type == "del" && strings.Contains(ln.CodeHTML, "-- old comment") {
			gotDel = true
		}
		if ln.Type == "add" && strings.Contains(ln.CodeHTML, "++ new thing") {
			gotAdd = true
		}
	}
	if !gotDel {
		t.Errorf("deleted '-- old comment' line was dropped/misclassified: %+v", files[0].Lines)
	}
	if !gotAdd {
		t.Errorf("added '++ new thing' line was dropped/misclassified: %+v", files[0].Lines)
	}
}

func TestHighlightLine(t *testing.T) {
	// TypeScript line → keyword/string spans matching the PR UI token classes.
	got := highlightLine("src/auth/oauth.ts", `const secret = await secrets.get('oauth/client_secret')`)
	for _, want := range []string{
		`<span class="kw">const</span>`,
		`<span class="kw">await</span>`,
		`class="str"`,
		`oauth/client_secret`,
	} {
		if !strings.Contains(got, want) {
			t.Errorf("highlightLine(ts) missing %q in %q", want, got)
		}
	}
	// Unknown extension → plain escape, no spans.
	plain := highlightLine("NOTES", `<b>hi</b>`)
	if plain != "&lt;b&gt;hi&lt;/b&gt;" {
		t.Errorf("highlightLine(unknown) = %q, want escaped plaintext", plain)
	}
	// Empty line stays empty.
	if highlightLine("a.go", "") != "" {
		t.Errorf("highlightLine(empty) should be empty")
	}
}

func TestHighlightFilesBackfillsPlain(t *testing.T) {
	files := []domain.FileDiff{{
		Path: "tetris.py",
		Lines: []domain.DiffLine{
			{Type: "hunk", CodeHTML: "Lines 1–3"},
			{Type: "add", New: "1", CodeHTML: "def draw_board(stdscr, game):"},
			{Type: "add", New: "2", CodeHTML: `<span class="kw">def</span> already()`},
		},
	}}
	if !HighlightFiles(files) {
		t.Fatal("HighlightFiles should report a change for plain lines")
	}
	if !strings.Contains(files[0].Lines[1].CodeHTML, `class="kw"`) {
		t.Errorf("plain python line should be highlighted, got %q", files[0].Lines[1].CodeHTML)
	}
	if files[0].Lines[2].CodeHTML != `<span class="kw">def</span> already()` {
		t.Errorf("already-highlighted line must be left alone, got %q", files[0].Lines[2].CodeHTML)
	}
	if HighlightFiles(files) {
		t.Error("second HighlightFiles pass should be a no-op")
	}
}

func TestHighlightFilesTotalBudget(t *testing.T) {
	const nfiles = 9
	files := make([]domain.FileDiff, nfiles)
	for i := range files {
		lines := make([]domain.DiffLine, maxHighlightLines)
		for j := range lines {
			lines[j] = domain.DiffLine{Type: "add", New: fmt.Sprintf("%d", j+1), CodeHTML: "package main"}
		}
		files[i] = domain.FileDiff{Path: fmt.Sprintf("f%d.go", i), Lines: lines}
	}
	HighlightFiles(files)
	highlighted := 0
	for _, f := range files {
		for _, l := range f.Lines {
			if strings.Contains(l.CodeHTML, `<span class="`) {
				highlighted++
			}
		}
	}
	if highlighted != maxHighlightTotalLines {
		t.Errorf("HighlightFiles highlighted %d lines, want %d", highlighted, maxHighlightTotalLines)
	}
}

func TestDiffWorkingVsBaseSkipsChroma(t *testing.T) {
	repo := initRepo(t)
	ctx := context.Background()
	gitInDir(t, repo, "checkout", "-b", "recur/feature")
	write(t, repo, "main.go", "package main\n")
	commit(t, repo, "add go")
	write(t, repo, "main.go", "package main\nfunc F() {}\n")

	m := NewManager(repo, t.TempDir())
	pr, err := m.DiffPR(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffPR: %v", err)
	}
	if !fileDiffHasChroma(pr.Files) {
		t.Errorf("DiffPR(%s, main) CodeHTML has no chroma spans, want highlighted .go", repo)
	}
	got, err := m.DiffWorkingVsBase(ctx, repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase: %v", err)
	}
	if fileDiffHasChroma(got.Files) {
		t.Errorf("DiffWorkingVsBase(%s, main) CodeHTML has chroma spans, want plain escaped lines", repo)
	}
}

func fileDiffHasChroma(files []domain.FileDiff) bool {
	for _, f := range files {
		for _, l := range f.Lines {
			if strings.Contains(l.CodeHTML, `<span class="`) {
				return true
			}
		}
	}
	return false
}

func TestFormatHunkLabel(t *testing.T) {
	cases := []struct {
		line string
		want string
	}{
		{"@@ -173,12 +173,16 @@ class Game:", "Lines 173–188 · class Game:"},
		{"@@ -1,2 +1,2 @@", "Lines 1–2"},
		{"@@ -0,0 +1,1 @@", "Line 1"},
		{"@@ -5 +5 @@ def foo():", "Line 5 · def foo():"},
		{"@@ -10,3 +0,0 @@", "Lines 10–12"}, // pure deletion → old side
	}
	for _, c := range cases {
		h, ok := parseHunkHeader(c.line)
		if !ok {
			t.Fatalf("parseHunkHeader(%q) failed", c.line)
		}
		if got := formatHunkLabel(h); got != c.want {
			t.Errorf("formatHunkLabel(%q): got %q, want %q", c.line, got, c.want)
		}
	}
}

func TestParseUnifiedDiffEdgeCases(t *testing.T) {
	// Empty diff → no files, no panic.
	if got := ParseUnifiedDiffPlain(""); got != nil {
		t.Errorf("empty diff should parse to nil, got %+v", got)
	}

	// HTML escaping: an added line containing markup must be escaped in CodeHTML.
	diff := "diff --git a/x.html b/x.html\n" +
		"new file mode 100644\n" +
		"--- /dev/null\n" +
		"+++ b/x.html\n" +
		"@@ -0,0 +1,1 @@\n" +
		"+<script>alert('xss')</script>\n"
	files := ParseUnifiedDiffPlain(diff)
	if len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}
	if files[0].Status != "a" {
		t.Errorf("status: got %q, want a", files[0].Status)
	}
	var codeHTML string
	for _, ln := range files[0].Lines {
		if ln.Type == "add" {
			codeHTML = ln.CodeHTML
		}
	}
	// Highlighting may wrap tokens in spans, but angle brackets must stay escaped
	// so a raw <script> element can never appear in CodeHTML.
	if strings.Contains(codeHTML, "<script>") {
		t.Errorf("real diff code must be HTML-escaped, got raw markup: %q", codeHTML)
	}
	if !strings.Contains(codeHTML, "&lt;") || !strings.Contains(codeHTML, "&gt;") {
		t.Errorf("expected escaped angle brackets, got %q", codeHTML)
	}

	// Binary file → recorded with no rendered lines.
	bin := "diff --git a/img.png b/img.png\n" +
		"new file mode 100644\n" +
		"Binary files /dev/null and b/img.png differ\n"
	bfiles := ParseUnifiedDiffPlain(bin)
	if len(bfiles) != 1 {
		t.Fatalf("expected 1 binary file entry, got %d", len(bfiles))
	}
	if len(bfiles[0].Lines) != 0 {
		t.Errorf("binary file should have no diff lines, got %d", len(bfiles[0].Lines))
	}
	if bfiles[0].Path != "img.png" {
		t.Errorf("ParseUnifiedDiffPlain(binary).Path = %q, want img.png", bfiles[0].Path)
	}
	if bfiles[0].Status != "a" {
		t.Errorf("ParseUnifiedDiffPlain(binary).Status = %q, want a", bfiles[0].Status)
	}

	mod := "diff --git a/img.png b/img.png\n" +
		"index 1111111..2222222 100644\n" +
		"Binary files a/img.png and b/img.png differ\n"
	mfiles := ParseUnifiedDiffPlain(mod)
	if len(mfiles) != 1 {
		t.Fatalf("ParseUnifiedDiffPlain(modified binary) len = %d, want 1", len(mfiles))
	}
	if mfiles[0].Path != "img.png" || mfiles[0].Status != "m" || len(mfiles[0].Lines) != 0 {
		t.Errorf("ParseUnifiedDiffPlain(modified binary) = %+v, want path=img.png status=m no lines", mfiles[0])
	}

	ren := "diff --git a/old.sql b/new.sql\n" +
		"similarity index 100%\n" +
		"rename from old.sql\n" +
		"rename to new.sql\n"
	rfiles := ParseUnifiedDiffPlain(ren)
	if len(rfiles) != 1 {
		t.Fatalf("ParseUnifiedDiffPlain(rename) len = %d, want 1", len(rfiles))
	}
	if rfiles[0].Path != "new.sql" || rfiles[0].OldPath != "old.sql" || rfiles[0].Status != "r" {
		t.Errorf("ParseUnifiedDiffPlain(rename) = %+v, want path=new.sql oldPath=old.sql status=r", rfiles[0])
	}
	if rfiles[0].Diff.Add != 0 || rfiles[0].Diff.Del != 0 || len(rfiles[0].Lines) != 0 {
		t.Errorf("ParseUnifiedDiffPlain(rename) stats = +%d -%d lines=%d, want empty", rfiles[0].Diff.Add, rfiles[0].Diff.Del, len(rfiles[0].Lines))
	}
}

// gitInDir runs a git command in dir with a fixed identity (helper for the diff
// tests that need to create branches).
func gitInDir(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=test", "GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=test", "GIT_COMMITTER_EMAIL=test@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

// LogCommits reads every commit's add/del out of a single `git log --numstat`
// (it used to shell out to `git show --numstat` once per commit). Verify the
// numstat block still lands on the right commit when there is more than one.
func TestLogCommitsWithoutBaseListsHEAD(t *testing.T) {
	repo := initRepo(t)
	commits, err := NewManager(repo, t.TempDir()).LogCommits(context.Background(), repo, "")
	if err != nil {
		t.Fatalf("LogCommits: %v", err)
	}
	if len(commits) != 1 {
		t.Fatalf("LogCommits() empty base len = %d, want 1", len(commits))
	}
	if commits[0].Message != "initial" {
		t.Errorf("LogCommits()[0].Message = %q, want initial", commits[0].Message)
	}
}

func TestLogCommitsNumstatPerCommit(t *testing.T) {
	repo := initRepo(t)
	gitInDir(t, repo, "checkout", "-b", "recur/feature")

	write(t, repo, "one.txt", "a\n")
	commit(t, repo, "recur(Coder): iteration 1 — first")
	write(t, repo, "two.txt", "a\nb\nc\n")
	write(t, repo, "three.txt", "x\ny\n")
	commit(t, repo, "recur(Reviewer): iteration 2 — second")

	commits, err := NewManager(repo, t.TempDir()).LogCommits(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("LogCommits: %v", err)
	}
	if len(commits) != 2 {
		t.Fatalf("expected 2 commits, got %d: %+v", len(commits), commits)
	}
	// Newest first: the 5-line commit, then the 1-line commit.
	if commits[0].Diff.Add != 5 || commits[0].Diff.Del != 0 {
		t.Errorf("newest commit stat = %+v, want add=5 del=0", commits[0].Diff)
	}
	if commits[1].Diff.Add != 1 || commits[1].Diff.Del != 0 {
		t.Errorf("older commit stat = %+v, want add=1 del=0", commits[1].Diff)
	}
	if commits[0].AgentCode != "RV" || commits[1].AgentCode != "CO" {
		t.Errorf("roles mismatched with stats: %q / %q", commits[0].AgentCode, commits[1].AgentCode)
	}
}

func TestLogCommitsAuthorWhenAndSHA(t *testing.T) {
	repo := initRepo(t)
	gitInDir(t, repo, "checkout", "-b", "recur/feature")
	write(t, repo, "tab.txt", "on\n")
	cmd := exec.Command("git", "add", "-A")
	cmd.Dir = repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git add: %v\n%s", err, out)
	}
	cmd = exec.Command("git", "commit", "-m", "Enable the forwarding deposit tab", "-m", "Turn the tab on by default\nin the bridge app.")
	cmd.Dir = repo
	cmd.Env = append(os.Environ(),
		"GIT_AUTHOR_NAME=mycodecrafting", "GIT_AUTHOR_EMAIL=me@example.com",
		"GIT_COMMITTER_NAME=mycodecrafting", "GIT_COMMITTER_EMAIL=me@example.com",
	)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git commit: %v\n%s", err, out)
	}

	commits, err := NewManager(repo, t.TempDir()).LogCommits(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("LogCommits: %v", err)
	}
	if len(commits) != 1 {
		t.Fatalf("LogCommits() len = %d, want 1", len(commits))
	}
	c := commits[0]
	if c.Author != "mycodecrafting" || c.AuthorRole != "Author" {
		t.Errorf("LogCommits() author/role = %q/%q, want mycodecrafting/Author", c.Author, c.AuthorRole)
	}
	if c.AgentCode != "MY" {
		t.Errorf("LogCommits() AgentCode = %q, want MY (author initials, not Maestro)", c.AgentCode)
	}
	if c.When == "" {
		t.Error("LogCommits() When is empty, want a relative time")
	}
	if len(c.SHA) != 40 {
		t.Errorf("LogCommits() SHA = %q, want 40-char full hash", c.SHA)
	}
	if c.Hash == "" || c.Hash == c.SHA {
		t.Errorf("LogCommits() Hash = %q, want short hash distinct from SHA", c.Hash)
	}
	if c.Message != "Enable the forwarding deposit tab" {
		t.Errorf("LogCommits() Message = %q, want subject only", c.Message)
	}
	if !strings.Contains(c.Body, "Turn the tab on by default") || !strings.Contains(c.Body, "bridge app") {
		t.Errorf("LogCommits() Body = %q, want expanded message", c.Body)
	}
}

func TestClassifyCommitMessage(t *testing.T) {
	tests := []struct {
		msg, author, role, code string
	}{
		{"fix typo", "Ada", "Author", "AD"},
		{"wip", "Josh Smith", "Author", "JS"},
		{"hello", "mycodecrafting", "Author", "MY"},
		{"", "   ", "Author", "?"},
		{"recur(Coder): iteration 1 — add a feature", "Recur", "Coder", "CO"},
		{"recur(Reviewer): iteration 2 — nits", "Recur", "Reviewer", "RV"},
	}
	for _, tt := range tests {
		role, code, _, _ := classifyCommitMessage(tt.msg, tt.author)
		if role != tt.role || code != tt.code {
			t.Errorf("classifyCommitMessage(%q, %q) = %q/%q, want %q/%q",
				tt.msg, tt.author, role, code, tt.role, tt.code)
		}
	}
}

func TestChangeStatusMatchesDiffWorkingVsBasePaths(t *testing.T) {
	repo := initRepo(t)
	write(t, repo, "extra.txt", "delete me\n")
	write(t, repo, "old.txt", "rename me\n")
	if err := os.WriteFile(filepath.Join(repo, "img.bin"), []byte{0x00, 0x01, 0xff, 0x89}, 0o644); err != nil {
		t.Fatal(err)
	}
	commit(t, repo, "base files")
	gitInDir(t, repo, "checkout", "-b", "recur/feat")

	write(t, repo, "README.md", "hello\nchanged\n")
	write(t, repo, "added.go", "package a\n")
	gitInDir(t, repo, "add", "added.go")
	if err := os.Remove(filepath.Join(repo, "extra.txt")); err != nil {
		t.Fatal(err)
	}
	gitInDir(t, repo, "mv", "old.txt", "new.txt")
	write(t, repo, "untracked.go", "package u\n")
	if err := os.WriteFile(filepath.Join(repo, "img.bin"), []byte{0x00, 0x02, 0xff, 0x89, 0x0a}, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "new.bin"), []byte{0, 1, 2, 3, 0}, 0o644); err != nil {
		t.Fatal(err)
	}

	st := ChangeStatus(context.Background(), repo, "main")
	pr, err := NewManager(repo, t.TempDir()).DiffWorkingVsBase(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase(%s, main) error = %v", repo, err)
	}
	got := map[string]bool{}
	for _, f := range pr.Files {
		got[f.Path] = true
	}
	for p := range st {
		if !got[p] {
			t.Errorf("ChangeStatus(%s, main) has %q (%s), DiffWorkingVsBase missing it; files=%v",
				repo, p, st[p], diffPaths(pr.Files))
		}
	}
	for p := range got {
		if _, ok := st[p]; !ok {
			t.Errorf("DiffWorkingVsBase(%s, main) has %q, ChangeStatus missing it; status=%#v",
				repo, p, st)
		}
	}
}

func TestDiffWorkingVsBaseUntrackedOverflowKeepsPaths(t *testing.T) {
	repo := initRepo(t)
	const n = untrackedMaxFiles + 10
	for i := 0; i < n; i++ {
		write(t, repo, fmt.Sprintf("u/%03d.txt", i), "x\n")
	}
	st := ChangeStatus(context.Background(), repo, "main")
	pr, err := NewManager(repo, t.TempDir()).DiffWorkingVsBase(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase(%s, main) error = %v", repo, err)
	}
	got := map[string]bool{}
	omitted := 0
	for _, f := range pr.Files {
		got[f.Path] = true
		if f.Status == "u" && len(f.Lines) == 1 && strings.Contains(f.Lines[0].CodeHTML, "too many untracked") {
			omitted++
		}
	}
	for p := range st {
		if !got[p] {
			t.Errorf("ChangeStatus(%s, main) has %q, DiffWorkingVsBase dropped it after untracked overflow", repo, p)
		}
	}
	if omitted == 0 {
		t.Errorf("DiffWorkingVsBase(%s, main) omitted-note rows = 0, want >0 for %d untracked files", repo, n)
	}
}

func TestDiffWorkingVsBaseUntrackedPathCapSignalsTruncation(t *testing.T) {
	repo := initRepo(t)
	n := untrackedMaxPaths + 2
	for i := 0; i < n; i++ {
		write(t, repo, fmt.Sprintf("u/%04d.txt", i), "x\n")
	}
	pr, err := NewManager(repo, t.TempDir()).DiffWorkingVsBase(context.Background(), repo, "")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase: %v", err)
	}
	if !pr.Truncated {
		t.Fatal("Truncated = false, want true when untracked paths exceed the cap")
	}
	listed := 0
	for _, f := range pr.Files {
		if strings.HasPrefix(f.Path, "u/") {
			listed++
		}
	}
	if listed != untrackedMaxPaths {
		t.Errorf("listed untracked = %d, want cap %d", listed, untrackedMaxPaths)
	}
}

func TestDiffWorkingVsBaseUntrackedByteBudget(t *testing.T) {
	repo := initRepo(t)
	chunk := strings.Repeat("x", maxSyntheticAddBytes)
	const n = 6
	for i := 0; i < n; i++ {
		write(t, repo, fmt.Sprintf("big/%d.txt", i), chunk)
	}
	pr, err := NewManager(repo, t.TempDir()).DiffWorkingVsBase(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase: %v", err)
	}
	omitted := 0
	withBody := 0
	for _, f := range pr.Files {
		if strings.HasPrefix(f.Path, "big/") && f.Status == "u" && len(f.Lines) == 1 && strings.Contains(f.Lines[0].CodeHTML, "too many untracked") {
			omitted++
		}
		if strings.HasPrefix(f.Path, "big/") && len(f.Lines) > 1 {
			withBody++
		}
	}
	if omitted == 0 {
		t.Errorf("byte-budget omitted rows = 0, want >0 for %d × %d-byte untracked files", n, maxSyntheticAddBytes)
	}
	if withBody == 0 {
		t.Errorf("byte-budget synthesized bodies = 0, want some files under the 2 MiB cap")
	}
}

func TestDiffWorkingVsBaseUntrackedBinaryHasNoLines(t *testing.T) {
	repo := initRepo(t)
	write(t, repo, "pic.png", "\x00PNG\x89binary")
	pr, err := NewManager(repo, t.TempDir()).DiffWorkingVsBase(context.Background(), repo, "main")
	if err != nil {
		t.Fatalf("DiffWorkingVsBase: %v", err)
	}
	var pic *domain.FileDiff
	for i := range pr.Files {
		if pr.Files[i].Path == "pic.png" {
			pic = &pr.Files[i]
			break
		}
	}
	if pic == nil {
		t.Fatal("DiffWorkingVsBase missing untracked pic.png")
	}
	if pic.Status != "a" {
		t.Errorf("pic.png status=%q want a", pic.Status)
	}
	if len(pic.Lines) != 0 {
		t.Errorf("untracked binary should have no diff lines, got %d", len(pic.Lines))
	}
}

func diffPaths(files []domain.FileDiff) []string {
	out := make([]string, len(files))
	for i, f := range files {
		out[i] = f.Path
	}
	return out
}
