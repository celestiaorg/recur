package git

import (
	"bytes"
	"context"
	"fmt"
	"html"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"recur/internal/domain"
)

// DiffStatVsBase returns rolled-up additions/deletions of the worktree versus
// baseBranch, including uncommitted changes to tracked files and line counts of
// untracked (non-ignored) text files (sidebar +N −M). An empty
// baseBranch means uncommitted-vs-HEAD (main working copy). When baseBranch is
// set, the merge-base with HEAD is preferred so only this branch's own changes
// count. Best-effort: binary files contribute 0 via numstat / content sniff.
func DiffStatVsBase(ctx context.Context, worktree, baseBranch string) (add, del int, err error) {
	worktree = strings.TrimSpace(worktree)
	if worktree == "" {
		return 0, 0, fmt.Errorf("diffstat: empty worktree")
	}
	base := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		base = b
		cmd := Command(ctx, "-C", worktree, "merge-base", b, "HEAD")
		// stdout only — CombinedOutput would interleave stderr warnings into the SHA.
		if out, merr := cmd.Output(); merr == nil {
			if mb := strings.TrimSpace(string(out)); mb != "" {
				base = mb
			}
		}
	}
	// Working tree (index + unstaged) vs base — matches what humans see as the
	// branch's outstanding change size, not only committed HEAD.
	out, err := Command(ctx, "-C", worktree, "diff", "--numstat", base).Output()
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok && len(ee.Stderr) > 0 {
			msg := strings.TrimSpace(string(ee.Stderr))
			if msg != "" {
				return 0, 0, fmt.Errorf("git diff --numstat: %s: %w", msg, err)
			}
		}
		return 0, 0, fmt.Errorf("git diff --numstat: %w", err)
	}
	add, del = parseNumstat(string(out))
	add += untrackedAddLines(ctx, worktree)
	return add, del, nil
}

// Caps so a dirty worktree (pre-gitignore build output, data dumps) cannot blow
// past the syncWorkspaceDiff deadline. Partial counts are preferred over stalls.
const (
	untrackedMaxFiles = 200
	untrackedMaxPaths = 2000
	untrackedMaxBytes = 2 << 20   // 2 MiB across all untracked files
	untrackedFileCap  = 512 << 10 // 512 KiB per file
)

// untrackedAddLines sums newline counts of untracked, non-ignored text files so
// newly created agent files show in the sidebar before they are staged. Honors
// ctx cancellation and hard caps so the scan stays inside the caller's budget.
func untrackedAddLines(ctx context.Context, worktree string) int {
	out, err := Command(ctx, "-C", worktree, "ls-files", "-z", "--others", "--exclude-standard").Output()
	if err != nil || len(out) == 0 {
		return 0
	}
	total, files, bytesRead := 0, 0, 0
	for _, rel := range bytes.Split(out, []byte{0}) {
		if len(rel) == 0 {
			continue
		}
		if err := ctx.Err(); err != nil {
			break
		}
		if files >= untrackedMaxFiles || bytesRead >= untrackedMaxBytes {
			break
		}
		n, read, ok := countTextFileLines(filepath.Join(worktree, string(rel)), untrackedFileCap)
		bytesRead += read
		if ok {
			total += n
			files++
		}
	}
	return total
}

// countTextFileLines returns the number of lines in path (at least 1 for a
// non-empty file with no trailing newline) and bytesRead (capped). Binary /
// unreadable files return ok=false.
func countTextFileLines(path string, maxBytes int) (lines, bytesRead int, ok bool) {
	f, err := os.Open(path)
	if err != nil {
		return 0, 0, false
	}
	defer f.Close()

	limited := io.LimitReader(f, int64(maxBytes))
	data, err := io.ReadAll(limited)
	if err != nil {
		return 0, len(data), false
	}
	bytesRead = len(data)
	if len(data) == 0 {
		return 0, 0, true // empty file: +0 (matches git's empty-blob numstat)
	}
	if bytes.IndexByte(data, 0) >= 0 || !utf8.Valid(data) {
		return 0, bytesRead, false
	}
	n := bytes.Count(data, []byte{'\n'})
	if data[len(data)-1] != '\n' {
		n++
	}
	return n, bytesRead, true
}

// DiffStats is a parsed (additions, deletions) pair for a single file or a whole
// diff. Kept tiny and local so the diff parser never depends on the domain package
// for its intermediate shapes.
type DiffStats struct {
	Add int
	Del int
}

// PRDiff is the PR-wide diff vs the base branch (PRD §11, §12.2): the list of
// changed files with parsed hunks and the rolled-up additions/deletions. It is
// computed via `git diff <base>...<branch>` (the three-dot form, so only the
// branch's own changes show — the symmetric merge-base, matching how GitHub
// renders a PR).
type PRDiff struct {
	Files     []domain.FileDiff
	Add       int
	Del       int
	Truncated bool // untracked path list hit untrackedMaxPaths; omitted files are not in Files
}

// CommitsBehind returns how many commits baseBranch is ahead of the worktree HEAD
// (`git rev-list --count HEAD..<base>`). 0 means the PR branch already contains
// every commit on base (up to date with the local base tip). Used for the PR
// "out of date" indicator — does not fetch remotes; callers that need a fresher
// remote tip should fetch first.
func (m *Manager) CommitsBehind(ctx context.Context, worktree, baseBranch string) (int, error) {
	if worktree == "" || baseBranch == "" {
		return 0, fmt.Errorf("commits-behind: worktree and base are required")
	}
	return m.revListCount(ctx, worktree, "HEAD.."+baseBranch)
}

// CommitsAhead returns how many commits the worktree HEAD is ahead of baseBranch
// (`git rev-list --count <base>..HEAD`). 0 means HEAD is the merge-base — nothing
// unique to land onto the base.
func (m *Manager) CommitsAhead(ctx context.Context, worktree, baseBranch string) (int, error) {
	if worktree == "" || baseBranch == "" {
		return 0, fmt.Errorf("commits-ahead: worktree and base are required")
	}
	return m.revListCount(ctx, worktree, baseBranch+"..HEAD")
}

func (m *Manager) revListCount(ctx context.Context, worktree, spec string) (int, error) {
	out, err := m.runIn(ctx, worktree, "rev-list", "--count", spec)
	if err != nil {
		return 0, fmt.Errorf("git rev-list %s: %w", spec, err)
	}
	n, err := strconv.Atoi(strings.TrimSpace(out))
	if err != nil {
		return 0, fmt.Errorf("parse rev-list count %q: %w", strings.TrimSpace(out), err)
	}
	if n < 0 {
		return 0, nil
	}
	return n, nil
}

// DiffPR returns the PR-wide diff of the loop branch against the base branch (PRD
// §11). It diffs the merge-base of base and HEAD against HEAD (`git diff
// <base>...HEAD`) so only the loop's own commits show. The unified diff is parsed
// into []domain.FileDiff with hunk/ctx/add/del lines; CodeHTML is
// HTML-escaped + syntax-highlighted (PRD §12.2). Robust to binary files,
// renames, and an empty diff (returns an empty PRDiff, never an error, for an
// up-to-date branch).
func (m *Manager) DiffPR(ctx context.Context, worktree, baseBranch string) (PRDiff, error) {
	// Resolve the base ref; prefer the merge-base so a branch behind base still
	// shows only its own changes. Fall back to the base name if merge-base fails.
	base := baseBranch
	if mb, err := m.runIn(ctx, worktree, "merge-base", baseBranch, "HEAD"); err == nil {
		if t := strings.TrimSpace(mb); t != "" {
			base = t
		}
	}

	out, err := m.runIn(ctx, worktree, "diff", "--no-color", base, "HEAD")
	if err != nil {
		return PRDiff{}, fmt.Errorf("git diff %s..HEAD: %w", baseBranch, err)
	}
	// Same capped highlight as CommitDiff — a large PR must not unbounded-chroma
	// every line while Files-changed waits on IPC.
	files := parseHighlightedPatch(out)
	pr := PRDiff{Files: files}
	for _, f := range files {
		pr.Add += f.Diff.Add
		pr.Del += f.Diff.Del
	}
	return pr, nil
}

// DiffWorkingVsBase is the Observatory files-changed view: the working tree
// (staged + unstaged tracked files, plus untracked text files) versus the
// merge-base with baseBranch. An empty baseBranch diffs versus HEAD — the main
// working copy's uncommitted delta. Unborn HEAD (git init, no commits) diffs
// versus the empty tree so the first staged files still appear as additions.
// Unlike DiffPR this includes work that is not committed yet, matching the
// explorer's Changed tab.
func (m *Manager) DiffWorkingVsBase(ctx context.Context, worktree, baseBranch string) (PRDiff, error) {
	worktree = strings.TrimSpace(worktree)
	if worktree == "" {
		return PRDiff{}, fmt.Errorf("diff: empty worktree")
	}
	against, err := workingDiffAgainst(ctx, worktree, baseBranch)
	if err != nil {
		return PRDiff{}, err
	}
	out, err := m.runIn(ctx, worktree, "diff", "--no-color", "--no-renames", "--end-of-options", against)
	if err != nil {
		return PRDiff{}, fmt.Errorf("git diff %s: %w", against, err)
	}
	// Plain parse — chroma over a 100-file working diff freezes the Observatory
	// review RPC (and the 1.6s live refresh). DiffFile still renders escaped
	// CodeHTML; PR/commit diffs keep parseHighlightedPatch.
	files := ParseUnifiedDiffPlain(out)
	seen := make(map[string]bool, len(files))
	for _, f := range files {
		seen[f.Path] = true
	}
	var extra []domain.FileDiff
	bodies, bytesRead := 0, 0
	untracked, listTruncated := untrackedRelPaths(ctx, worktree)
	for _, rel := range untracked {
		if seen[rel] {
			continue
		}
		if bodies >= untrackedMaxFiles || bytesRead >= untrackedMaxBytes {
			extra = append(extra, omittedUntrackedDiff(rel))
			continue
		}
		abs := filepath.Join(worktree, filepath.FromSlash(rel))
		body, truncated, rerr := readCapped(abs, maxSyntheticAddBytes)
		if rerr != nil {
			extra = append(extra, domain.FileDiff{Path: rel, Status: "u"})
			continue
		}
		bytesRead += len(body)
		extra = append(extra, syntheticAddDiff(rel, body, truncated))
		bodies++
	}
	if len(extra) > 0 {
		files = append(files, extra...)
	}
	pr := PRDiff{Files: files, Truncated: listTruncated}
	for _, f := range files {
		pr.Add += f.Diff.Add
		pr.Del += f.Diff.Del
	}
	return pr, nil
}

// workingDiffAgainst is the named tree DiffWorkingVsBase compares the working
// copy to. Empty baseBranch is HEAD, except when HEAD is unborn (`git init`,
// no commits) — then the empty tree, so staged files still show as additions.
func workingDiffAgainst(ctx context.Context, worktree, baseBranch string) (string, error) {
	against := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		against = MergeBase(ctx, worktree, b)
	}
	if against != "HEAD" || commitRefExists(ctx, worktree, "HEAD") {
		return against, nil
	}
	oid, err := emptyTreeOID(ctx, worktree)
	if err != nil {
		return "", fmt.Errorf("git diff HEAD: %w", err)
	}
	return oid, nil
}

func commitRefExists(ctx context.Context, worktree, ref string) bool {
	_, err := Command(ctx, "-C", worktree, "rev-parse", "--verify", "--quiet", ref+"^{commit}").Output()
	return err == nil
}

// emptyTreeOID writes (if needed) and returns this repo's empty-tree object id
// so `git diff` can run before the first commit. hash-object follows the
// repo's object format (SHA-1 or SHA-256).
func emptyTreeOID(ctx context.Context, worktree string) (string, error) {
	cmd := Command(ctx, "-C", worktree, "hash-object", "-w", "-t", "tree", "--stdin")
	cmd.Stdin = bytes.NewReader(nil)
	out, err := cmd.Output()
	if err != nil {
		return "", err
	}
	oid := strings.TrimSpace(string(out))
	if oid == "" {
		return "", fmt.Errorf("empty oid")
	}
	return oid, nil
}

// untrackedRelPaths lists untracked, non-ignored paths (repo-relative, slash
// separated). Capped at untrackedMaxPaths so a dirty tree of build output
// cannot blow the caller; DiffWorkingVsBase still only synthesizes bodies for
// the first untrackedMaxFiles of those. truncated is true when more untracked
// paths existed than were returned.
func untrackedRelPaths(ctx context.Context, worktree string) (paths []string, truncated bool) {
	out, err := Command(ctx, "-C", worktree, "ls-files", "-z", "--others", "--exclude-standard").Output()
	if err != nil || len(out) == 0 {
		return nil, false
	}
	for _, rel := range bytes.Split(out, []byte{0}) {
		if len(rel) == 0 {
			continue
		}
		if err := ctx.Err(); err != nil {
			break
		}
		if len(paths) >= untrackedMaxPaths {
			return paths, true
		}
		p := filepath.ToSlash(string(rel))
		if p == "" {
			continue
		}
		paths = append(paths, p)
	}
	return paths, false
}

// logCommitsHEADLimit caps working-copy history so `git log HEAD` on a long
// main branch doesn't dump thousands of rows into the Commits tab.
const logCommitsHEADLimit = 100

// LogCommits returns commits for the Observatory Commits tab, newest first.
// With a base branch this is unique-to-this-workspace (merge-base..HEAD).
// An empty base (main working copy) is recent HEAD history, capped.
// Each commit carries hash/message/author and a per-commit diff stat;
// AuthorRole/AgentCode/IterLabel/Color are derived from the message (legacy
// engine commits used "recur(<Role>): iteration N — …"; current ones are a
// plain summary, so role falls back to the git author). The per-commit FILE
// diffs are filled lazily by CommitDiff so the list stays cheap.
func (m *Manager) LogCommits(ctx context.Context, worktree, baseBranch string) ([]domain.Commit, error) {
	// %x1f between scalar fields, %x1d after the (possibly multiline) body so
	// numstat still starts at a known boundary. %x1e leads each commit.
	// --numstat + logNumstatFlags (--diff-merges=first-parent on Git 2.31+)
	// keep list stats aligned with CommitDiffFirstParent in one process.
	const fmtStr = "%x1e%h%x1f%H%x1f%an%x1f%ae%x1f%aI%x1f%s%x1f%b%x1d"
	args := []string{"log", "--no-color", "--numstat"}
	args = append(args, logNumstatFlags(Probe())...)
	args = append(args, "--format="+fmtStr)

	rev := "HEAD"
	if b := strings.TrimSpace(baseBranch); b != "" {
		base := b
		if mb, err := m.runIn(ctx, worktree, "merge-base", b, "HEAD"); err == nil {
			if t := strings.TrimSpace(mb); t != "" {
				base = t
			}
		}
		rev = base + "..HEAD"
	} else {
		args = append(args, "-n", strconv.Itoa(logCommitsHEADLimit))
	}
	args = append(args, rev)
	out, err := m.runIn(ctx, worktree, args...)
	if err != nil {
		return nil, fmt.Errorf("git log %s: %w", rev, err)
	}

	now := time.Now()
	var commits []domain.Commit
	for _, rec := range strings.Split(out, "\x1e") {
		if strings.TrimSpace(rec) == "" {
			continue
		}
		payload, numstat, _ := strings.Cut(rec, "\x1d")
		fields := strings.SplitN(payload, "\x1f", 7)
		if len(fields) < 6 {
			continue
		}
		hash := strings.TrimSpace(fields[0])
		sha := strings.TrimSpace(fields[1])
		author := strings.TrimSpace(fields[2])
		msg := strings.TrimSpace(fields[5])
		body := ""
		if len(fields) >= 7 {
			body = strings.TrimSpace(fields[6])
		}
		role, code, iterLabel, color := classifyCommitMessage(msg+"\n"+body, author)
		add, del := parseNumstat(numstat)
		commits = append(commits, domain.Commit{
			Hash:       hash,
			SHA:        sha,
			Message:    msg,
			Body:       body,
			Author:     author,
			AuthorRole: role,
			AgentCode:  code,
			Color:      color,
			IterLabel:  iterLabel,
			Diff:       domain.DiffStat{Add: add, Del: del},
			When:       formatCommitWhen(fields[4], now),
		})
	}
	return commits, nil
}

// CommitDiff returns one commit's own diff (`git show --format= <hash>`), parsed
// into []domain.FileDiff with escaped + highlighted code (PRD §11). Merge
// commits have no combined patch here — `git show` prints nothing without
// -m/--cc/--first-parent — so eager fill paths do not persist a whole-branch
// delta from a "sync with base" merge. The Commits-tab drill-in uses
// CommitDiffFirstParent instead.
func (m *Manager) CommitDiff(ctx context.Context, worktree, hash string) ([]domain.FileDiff, error) {
	return m.showCommitDiff(ctx, worktree, hash, false)
}

// CommitDiffFirstParent is git show --first-parent: merge commits get a unified
// diff against the first parent (GitHub's commit view). Non-merge and empty
// commits match CommitDiff.
func (m *Manager) CommitDiffFirstParent(ctx context.Context, worktree, hash string) ([]domain.FileDiff, error) {
	return m.showCommitDiff(ctx, worktree, hash, true)
}

func (m *Manager) showCommitDiff(ctx context.Context, worktree, hash string, firstParent bool) ([]domain.FileDiff, error) {
	args := []string{"show"}
	if firstParent {
		args = append(args, "--first-parent")
	}
	args = append(args, "--no-color", "--format=", hash)
	out, err := m.runIn(ctx, worktree, args...)
	if err != nil {
		return nil, fmt.Errorf("git show %s: %w", hash, err)
	}
	return parseHighlightedPatch(out), nil
}

// parseNumstat sums the additions/deletions in `--numstat` output (best effort:
// zeros on anything unparseable, including binary files, which report "-").
func parseNumstat(out string) (add, del int) {
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 2 {
			continue
		}
		if a, e := strconv.Atoi(fields[0]); e == nil {
			add += a
		}
		if d, e := strconv.Atoi(fields[1]); e == nil {
			del += d
		}
	}
	return add, del
}

// classifyCommitMessage derives the descriptive author role, 2-letter agent code,
// an iteration label, and a UI color from a commit message + author name. Legacy
// engine commits used "recur(<Role>): iteration N — …"; current ones are a plain
// summary, so role falls back to the git author's initials (not Maestro's MA).
func classifyCommitMessage(msg, authorName string) (role, code, iterLabel, color string) {
	role, code, color = "Author", authorInitials(authorName), "var(--indigo)"
	low := strings.ToLower(msg)
	switch {
	case strings.Contains(low, "security reviewer"):
		role, code, color = "Security Reviewer", "SR", "var(--coral)"
	case strings.Contains(low, "reviewer"):
		role, code, color = "Reviewer", "RV", "var(--indigo)"
	case strings.Contains(low, "tester"):
		role, code, color = "Tester", "TE", "var(--emerald)"
	case strings.Contains(low, "coder"):
		role, code, color = "Coder", "CO", "var(--cyan)"
	}
	// Iteration label: "iteration N" anywhere in the message. Slice the digit run
	// out of the message rather than growing a string one rune at a time.
	if i := strings.Index(low, "iteration "); i >= 0 {
		rest := low[i+len("iteration "):]
		n := 0
		for n < len(rest) && rest[n] >= '0' && rest[n] <= '9' {
			n++
		}
		if n > 0 {
			iterLabel = "iteration " + rest[:n]
		}
	}
	return role, code, iterLabel, color
}

// authorInitials is the 2-letter avatar for a git author who is not a Recur
// agent: first letters of the first and last words, or the first two letters of
// a single token. Empty names become "?".
func authorInitials(name string) string {
	fields := strings.Fields(strings.TrimSpace(name))
	if len(fields) == 0 {
		return "?"
	}
	takeLetters := func(s string, n int) []rune {
		out := make([]rune, 0, n)
		for _, r := range s {
			if unicode.IsLetter(r) || unicode.IsNumber(r) {
				out = append(out, unicode.ToUpper(r))
				if len(out) == n {
					break
				}
			}
		}
		return out
	}
	var letters []rune
	if len(fields) == 1 {
		letters = takeLetters(fields[0], 2)
	} else {
		letters = takeLetters(fields[0], 1)
		letters = append(letters, takeLetters(fields[len(fields)-1], 1)...)
	}
	if len(letters) == 0 {
		return "?"
	}
	return string(letters)
}

func formatCommitWhen(iso string, now time.Time) string {
	t, err := time.Parse(time.RFC3339, strings.TrimSpace(iso))
	if err != nil {
		return "now"
	}
	d := now.Sub(t)
	if d < 0 {
		d = 0
	}
	if d < time.Minute {
		return "now"
	}
	if d < time.Hour {
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	if d < 24*time.Hour {
		return fmt.Sprintf("%dh", int(d.Hours()))
	}
	return fmt.Sprintf("%dd", int(d.Hours()/24))
}

// ParseUnifiedDiffPlain is the fast path: HTML-escaped code only (no chroma).
// Callers that want highlighting can run HighlightFiles afterward (capped).
func ParseUnifiedDiffPlain(diff string) []domain.FileDiff {
	return parseUnifiedDiff(diff)
}

// parseHighlightedPatch escapes first, then chroma-highlights with HighlightFiles'
// per-file cap. Use this for UI-facing git diff/show so a large commit cannot
// unbounded-highlight while the renderer waits on IPC.
func parseHighlightedPatch(diff string) []domain.FileDiff {
	files := ParseUnifiedDiffPlain(diff)
	HighlightFiles(files)
	return files
}

func parseUnifiedDiff(diff string) []domain.FileDiff {
	var files []domain.FileDiff
	var cur *domain.FileDiff
	var oldLine, newLine int
	// inHunk tracks whether we're inside a hunk body (after a "@@" line). The
	// "--- "/"+++ " FILE headers only appear in a file's pre-hunk header block; once
	// inside a hunk, a deleted line whose content starts with "-- " (e.g. a SQL/Lua
	// comment) renders as "--- ..." and an added "++ " line as "+++ ..." — those must
	// be classified as del/add CONTENT, not headers, or the row is dropped and the
	// line numbers desync. We only honor the header cases when !inHunk.
	var inHunk bool

	flush := func() {
		if cur != nil {
			files = append(files, *cur)
		}
		cur = nil
	}

	lines := strings.Split(diff, "\n")
	for i := 0; i < len(lines); i++ {
		line := lines[i]
		switch {
		case strings.HasPrefix(line, "diff --git "):
			flush()
			path := pathFromDiffHeader(line)
			cur = &domain.FileDiff{Path: path, Status: "m"}
			oldLine, newLine = 0, 0
			inHunk = false

		case cur == nil:
			// Lines before the first "diff --git" header (none in normal output).
			continue

		case strings.HasPrefix(line, "new file mode"):
			cur.Status = "a"
		case strings.HasPrefix(line, "deleted file mode"):
			cur.Status = "d"
		case strings.HasPrefix(line, "rename from "):
			cur.Status = "r"
			cur.OldPath = strings.TrimSpace(strings.TrimPrefix(line, "rename from "))
		case strings.HasPrefix(line, "rename to "):
			// Prefer the post-rename path for display.
			cur.Path = strings.TrimSpace(strings.TrimPrefix(line, "rename to "))
		case strings.HasPrefix(line, "Binary files "):
			// A binary file: record the change with no rendered lines.
			cur.Lines = nil

		case !inHunk && strings.HasPrefix(line, "--- "):
			// Old-file header (pre-hunk only); the new-file path is authoritative.
		case !inHunk && strings.HasPrefix(line, "+++ "):
			if p := pathFromFileHeader(line); p != "" {
				cur.Path = p
			}

		case strings.HasPrefix(line, "@@"):
			h, ok := parseHunkHeader(line)
			if !ok {
				continue
			}
			oldLine, newLine = h.oldStart, h.newStart
			inHunk = true
			cur.Lines = append(cur.Lines, domain.DiffLine{
				Type: "hunk", CodeHTML: html.EscapeString(formatHunkLabel(h)),
			})

		case strings.HasPrefix(line, "+"):
			cur.Lines = append(cur.Lines, domain.DiffLine{
				New: strconv.Itoa(newLine), Type: "add", CodeHTML: html.EscapeString(line[1:]),
			})
			cur.Diff.Add++
			newLine++

		case strings.HasPrefix(line, "-"):
			cur.Lines = append(cur.Lines, domain.DiffLine{
				Old: strconv.Itoa(oldLine), Type: "del", CodeHTML: html.EscapeString(line[1:]),
			})
			cur.Diff.Del++
			oldLine++

		case strings.HasPrefix(line, " "):
			cur.Lines = append(cur.Lines, domain.DiffLine{
				Old: strconv.Itoa(oldLine), New: strconv.Itoa(newLine),
				Type: "ctx", CodeHTML: html.EscapeString(line[1:]),
			})
			oldLine++
			newLine++

		case line == `\ No newline at end of file`:
			// Informational git marker — not a rendered diff row.

		default:
			// "index ...", "old mode"/"new mode", and other metadata: ignore.
		}
	}
	flush()
	return files
}

// pathFromDiffHeader extracts a display path from a "diff --git a/x b/y" header,
// preferring the b/ (new) side. Falls back to the a/ side for a deletion.
func pathFromDiffHeader(line string) string {
	rest := strings.TrimPrefix(line, "diff --git ")
	// Split on " b/" to find the new path; quoted/renamed paths are uncommon for
	// engine commits, so the simple form is sufficient and robust here.
	if i := strings.Index(rest, " b/"); i >= 0 {
		return strings.TrimSpace(rest[i+len(" b/"):])
	}
	// Fallback: strip a leading "a/".
	if strings.HasPrefix(rest, "a/") {
		half := rest
		if sp := strings.IndexByte(rest, ' '); sp >= 0 {
			half = rest[:sp]
		}
		return strings.TrimPrefix(half, "a/")
	}
	return strings.TrimSpace(rest)
}

// pathFromFileHeader extracts the path from a "+++ b/path" header. Returns "" for
// "+++ /dev/null" (a deletion's new side).
func pathFromFileHeader(line string) string {
	p := strings.TrimSpace(strings.TrimPrefix(line, "+++ "))
	if p == "/dev/null" {
		return ""
	}
	p = strings.TrimPrefix(p, "b/")
	return p
}

// hunkHeader is the parsed form of a unified-diff "@@ … @@" line.
type hunkHeader struct {
	oldStart, oldCount int
	newStart, newCount int
	context            string // trailing function/context label after the second @@
}

// parseHunkHeader parses "@@ -<oldStart>[,<oldCount>] +<newStart>[,<newCount>] @@ …"
// into starts, counts, and optional trailing context. ok=false if malformed.
// A missing count defaults to 1 (git's unified-diff convention).
func parseHunkHeader(line string) (hunkHeader, bool) {
	body := line
	if i := strings.Index(line, "@@ "); i >= 0 {
		body = line[i+len("@@ "):]
	}
	end := strings.Index(body, " @@")
	if end < 0 {
		return hunkHeader{}, false
	}
	h := hunkHeader{oldCount: 1, newCount: 1, context: strings.TrimSpace(body[end+len(" @@"):])}
	for _, r := range strings.Fields(body[:end]) {
		switch {
		case strings.HasPrefix(r, "-"):
			h.oldStart, h.oldCount = rangePair(r[1:])
		case strings.HasPrefix(r, "+"):
			h.newStart, h.newCount = rangePair(r[1:])
		}
	}
	return h, true
}

// formatHunkLabel turns a parsed hunk into a GitHub-style UI label, e.g.
// "Lines 173–188 · class Game:" — never the raw "@@ -173,12 +173,16 @@" chrome.
func formatHunkLabel(h hunkHeader) string {
	label := lineRangeLabel(h)
	if h.context != "" {
		return label + " · " + h.context
	}
	return label
}

func lineRangeLabel(h hunkHeader) string {
	// Prefer the new-file side (what the reviewer is looking at); fall back to old
	// for pure deletions (newCount == 0).
	start, count := h.newStart, h.newCount
	if count == 0 {
		start, count = h.oldStart, h.oldCount
	}
	if count <= 1 {
		return fmt.Sprintf("Line %d", start)
	}
	return fmt.Sprintf("Lines %d–%d", start, start+count-1)
}

// rangePair parses "<start>[,<count>]" into start and count (count defaults to 1).
func rangePair(r string) (start, count int) {
	count = 1
	if i := strings.IndexByte(r, ','); i >= 0 {
		start = atoiOr(r[:i], 0)
		count = atoiOr(r[i+1:], 1)
		return start, count
	}
	return atoiOr(r, 0), 1
}

func atoiOr(s string, fallback int) int {
	n, err := strconv.Atoi(s)
	if err != nil {
		return fallback
	}
	return n
}
