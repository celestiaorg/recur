package observe

import (
	"bufio"
	"hash/fnv"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode"

	"recur/internal/code"
	"recur/internal/domain"
)

const (
	dupWindow     = 5  // consecutive significant lines that form a clone fingerprint
	dupMinLineLen = 12 // ignore tiny normalized lines
	dupMaxClones  = 24
	dupMaxOccs    = 8
)

// fileSeq is one file's duplication fingerprint: the 1-based line numbers of
// its significant lines and a hash per line.
//
// Deliberately no preview text: keeping an 80-char preview per significant line
// of the whole repo was a second, unbounded copy of the codebase in memory for
// a report that shows at most dupMaxClones × dupMaxOccs snippets. Previews are
// read back from disk for the handful of sites that actually get reported.
type fileSeq struct {
	path    string
	lineNos []int // 1-based line numbers of significant lines
	hashes  []uint64
}

type dupSite struct {
	path  string
	start int // index into fileSeq arrays
}

// CollectMetrics walks root and computes size/complexity/duplication proxies.
func CollectMetrics(root string) domain.MetricSnapshot {
	var m domain.MetricSnapshot
	var seqs []fileSeq

	_ = filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if skipDir(d.Name()) {
				return filepath.SkipDir
			}
			return nil
		}
		if !isSource(d.Name()) {
			return nil
		}
		rel, _ := filepath.Rel(root, path)
		rel = filepath.ToSlash(rel)
		f, err := os.Open(path)
		if err != nil {
			return nil
		}
		defer f.Close()
		m.Files++
		if isTestFile(d.Name()) {
			m.TestFiles++
		}
		sc := bufio.NewScanner(f)
		sc.Buffer(make([]byte, 64*1024), 1024*1024)
		loc := 0
		complexity := 1
		todos := 0
		var seq fileSeq
		seq.path = rel
		lineNo := 0
		for sc.Scan() {
			lineNo++
			line := sc.Text()
			trimmed := strings.TrimSpace(line)
			if trimmed == "" || strings.HasPrefix(trimmed, "//") || strings.HasPrefix(trimmed, "#") || strings.HasPrefix(trimmed, "*") {
				continue
			}
			loc++
			complexity += branchWeight(trimmed)
			if strings.Contains(trimmed, "TODO") || strings.Contains(trimmed, "FIXME") || strings.Contains(trimmed, "HACK") {
				todos++
			}
			norm := normalizeLine(trimmed)
			if len(norm) >= dupMinLineLen && !isNoiseLine(norm) {
				seq.lineNos = append(seq.lineNos, lineNo)
				seq.hashes = append(seq.hashes, lineHash(norm))
			}
		}
		m.LOC += loc
		m.TodoCount += todos
		if complexity > m.MaxComplexity {
			m.MaxComplexity = complexity
		}
		reason := ""
		if complexity >= 25 {
			reason = "high complexity"
		} else if loc >= 400 {
			reason = "large file"
		}
		if reason != "" {
			m.Hotspots = append(m.Hotspots, domain.Hotspot{
				Path: rel, LOC: loc, Complexity: complexity, Reason: reason,
			})
		}
		if len(seq.hashes) >= dupWindow {
			seqs = append(seqs, seq)
		}
		return nil
	})

	if m.Files > 0 {
		totalC := 0
		if len(m.Hotspots) > 0 {
			for _, h := range m.Hotspots {
				totalC += h.Complexity
			}
			m.AvgComplexity = float64(totalC) / float64(len(m.Hotspots))
		} else {
			m.AvgComplexity = float64(m.MaxComplexity) * 0.35
		}
	}

	clones, covered := findClones(root, seqs)
	m.Duplicates = clones
	if m.LOC > 0 {
		m.Duplication = float64(covered) / float64(m.LOC)
		if m.Duplication > 1 {
			m.Duplication = 1
		}
	}
	if len(m.Hotspots) > 8 {
		m.Hotspots = m.Hotspots[:8]
	}
	return m
}

func findClones(root string, seqs []fileSeq) ([]domain.DupClone, int) {
	groups := map[uint64][]dupSite{}
	for _, seq := range seqs {
		for i := 0; i+dupWindow <= len(seq.hashes); i++ {
			key := windowHash(seq.hashes[i : i+dupWindow])
			groups[key] = append(groups[key], dupSite{path: seq.path, start: i})
		}
	}

	type rawClone struct {
		sites []dupSite
		score int
	}
	var raw []rawClone
	for _, sites := range groups {
		uniq := dedupeSites(sites)
		if len(uniq) < 2 || !hasDistinctCloneSites(uniq) {
			continue
		}
		raw = append(raw, rawClone{sites: uniq, score: len(uniq) * dupWindow})
	}
	byPath := map[string]fileSeq{}
	for _, s := range seqs {
		byPath[s.path] = s
	}
	// Tiebreak on the first site's location: previews are no longer held in
	// memory, and path:line is just as stable a total order.
	sort.Slice(raw, func(i, j int) bool {
		if raw[i].score != raw[j].score {
			return raw[i].score > raw[j].score
		}
		a, b := raw[i].sites[0], raw[j].sites[0]
		if a.path != b.path {
			return a.path < b.path
		}
		return a.start < b.start
	})

	covered := map[string]bool{}
	var clones []domain.DupClone
	seenOcc := map[string]bool{}
	previews := newPreviewReader(root)

	for _, r := range raw {
		if len(clones) >= dupMaxClones {
			break
		}
		blockLen := expandClone(byPath, r.sites)
		occs := make([]domain.DupLoc, 0, len(r.sites))
		sigParts := make([]string, 0, len(r.sites))
		for _, s := range r.sites {
			seq := byPath[s.path]
			startLine := seq.lineNos[s.start]
			endIdx := s.start + blockLen - 1
			if endIdx >= len(seq.lineNos) {
				endIdx = len(seq.lineNos) - 1
			}
			endLine := seq.lineNos[endIdx]
			occs = append(occs, domain.DupLoc{
				Path: s.path,
				Line: startLine,
				End:  endLine,
				Text: previews.at(s.path, startLine),
			})
			sigParts = append(sigParts, s.path+":"+itoa(startLine))
			for i := s.start; i <= endIdx; i++ {
				covered[s.path+":"+itoa(seq.lineNos[i])] = true
			}
			if len(occs) >= dupMaxOccs {
				break
			}
		}
		sort.Strings(sigParts)
		sig := strings.Join(sigParts, "|") + "#" + itoa(blockLen)
		if seenOcc[sig] {
			continue
		}
		seenOcc[sig] = true
		first := byPath[r.sites[0].path]
		clones = append(clones, domain.DupClone{
			Lines:       blockLen,
			Preview:     previews.at(r.sites[0].path, first.lineNos[r.sites[0].start]),
			Occurrences: occs,
		})
	}
	return clones, len(covered)
}

// previewReader fetches the source text of specific reported lines, on demand.
// At most dupMaxClones × dupMaxOccs sites are ever reported, so this reads a
// bounded number of lines instead of keeping a preview for every significant
// line in the repo. Results are memoized because a hot clone group reports the
// same site twice (occurrence text and clone preview).
type previewReader struct {
	root  string
	cache map[string]string
}

func newPreviewReader(root string) *previewReader {
	return &previewReader{root: root, cache: map[string]string{}}
}

func (p *previewReader) at(rel string, line int) string {
	key := rel + ":" + itoa(line)
	if v, ok := p.cache[key]; ok {
		return v
	}
	out := ""
	if f, err := os.Open(filepath.Join(p.root, filepath.FromSlash(rel))); err == nil {
		sc := bufio.NewScanner(f)
		sc.Buffer(make([]byte, 64*1024), 1024*1024)
		for n := 1; sc.Scan(); n++ {
			if n == line {
				out = trimPreview(strings.TrimSpace(sc.Text()), 80)
				break
			}
		}
		f.Close() //nolint:errcheck
	}
	p.cache[key] = out
	return out
}

func expandClone(byPath map[string]fileSeq, sites []dupSite) int {
	n := dupWindow
	for n < 80 {
		var h uint64
		ok := true
		for i, st := range sites {
			seq := byPath[st.path]
			idx := st.start + n
			if idx >= len(seq.hashes) {
				ok = false
				break
			}
			if i == 0 {
				h = seq.hashes[idx]
			} else if seq.hashes[idx] != h {
				ok = false
				break
			}
		}
		if !ok {
			break
		}
		n++
	}
	return n
}

func dedupeSites(sites []dupSite) []dupSite {
	seen := map[string]bool{}
	var out []dupSite
	for _, s := range sites {
		k := s.path + ":" + itoa(s.start)
		if seen[k] {
			continue
		}
		seen[k] = true
		out = append(out, s)
	}
	return out
}

func hasDistinctCloneSites(sites []dupSite) bool {
	paths := map[string]int{}
	for _, s := range sites {
		paths[s.path]++
	}
	if len(paths) >= 2 {
		return true
	}
	// Same file: require non-overlapping windows (starts at least window apart).
	if len(sites) < 2 {
		return false
	}
	starts := make([]int, len(sites))
	for i, s := range sites {
		starts[i] = s.start
	}
	sort.Ints(starts)
	for i := 1; i < len(starts); i++ {
		if starts[i]-starts[i-1] >= dupWindow {
			return true
		}
	}
	return false
}

func windowHash(hs []uint64) uint64 {
	h := fnv.New64a()
	var b [8]byte
	for _, x := range hs {
		for i := 0; i < 8; i++ {
			b[i] = byte(x >> (8 * i))
		}
		_, _ = h.Write(b[:])
	}
	return h.Sum64()
}

func lineHash(norm string) uint64 {
	h := fnv.New64a()
	_, _ = h.Write([]byte(norm))
	return h.Sum64()
}

func isNoiseLine(norm string) bool {
	switch norm {
	case "returnnil", "returnnull", "return;", "pass", "break;", "continue;",
		"}else{", "elsenull", "end", "fi", "done":
		return true
	}
	// Pure punctuation / braces.
	for _, r := range norm {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			return false
		}
	}
	return true
}

func trimPreview(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func branchWeight(line string) int {
	n := 0
	for _, kw := range []string{
		"if ", "else if", "elif ", "for ", "while ", "case ", "catch ",
		"except ", "match ", "when ", "&&", "||", "?", "switch ",
	} {
		if strings.Contains(line, kw) {
			n++
		}
	}
	return n
}

func normalizeLine(s string) string {
	var b strings.Builder
	for _, r := range s {
		if unicode.IsSpace(r) {
			continue
		}
		b.WriteRune(unicode.ToLower(r))
	}
	return b.String()
}

func isSource(name string) bool {
	return code.IsSourceFile(name)
}

func isTestFile(name string) bool {
	n := strings.ToLower(name)
	return strings.HasSuffix(n, "_test.go") ||
		strings.HasSuffix(n, "_test.py") ||
		strings.HasSuffix(n, "_spec.rb") ||
		strings.HasSuffix(n, ".test.ts") ||
		strings.HasSuffix(n, ".test.tsx") ||
		strings.HasSuffix(n, ".test.js") ||
		strings.HasSuffix(n, ".test.jsx") ||
		strings.HasSuffix(n, ".spec.ts") ||
		strings.HasSuffix(n, ".spec.tsx") ||
		strings.HasSuffix(n, ".spec.js") ||
		strings.HasSuffix(n, ".spec.jsx") ||
		strings.HasSuffix(n, "test.java") ||
		strings.HasSuffix(n, "tests.java") ||
		strings.HasSuffix(n, "test.kt") ||
		strings.HasSuffix(n, "test.swift") ||
		strings.HasSuffix(n, "_test.rs") ||
		strings.HasPrefix(n, "test_") ||
		strings.Contains(n, "_test.") ||
		strings.Contains(n, ".test.") ||
		strings.Contains(n, ".spec.")
}

func skipDir(name string) bool {
	switch name {
	case ".git", "node_modules", "dist", "build", "vendor", ".next",
		"coverage", "__pycache__", ".pytest_cache", ".mypy_cache", ".tox",
		".venv", "venv", ".turbo", ".cache", "bin", "target",
		"Pods", ".gradle", ".idea", "out", "obj", "bindings":
		return true
	}
	return strings.HasPrefix(name, ".")
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var b [16]byte
	i := len(b)
	neg := n < 0
	if neg {
		n = -n
	}
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		b[i] = '-'
	}
	return string(b[i:])
}
