package code

import (
	"context"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"recur/internal/domain"
	"recur/internal/rg"
)

// DefaultMaxIndexBytes is the default resident budget for all file indexes
// combined. ~64MiB holds a few medium trees or one large monorepo; cold
// workspaces are LRU-evicted when the budget is exceeded.
const DefaultMaxIndexBytes int64 = 64 << 20

// entryOverhead approximates per-path non-string cost (two string headers +
// map entry + slice slots) on amd64/arm64. Tuned for budgeting, not accounting.
const entryOverhead = 64

// FileIndex is an in-memory path list for Quick Open / @-mentions.
// Paths and Basenames are parallel arrays (same length, same order).
//
// Safe for concurrent readers via Snapshot; mutations hold mu.
type FileIndex struct {
	mu        sync.RWMutex
	root      string
	paths     []string
	basenames []string
	byPath    map[string]int // path → index in paths
	gen       uint64
	building  bool
	ready     bool // at least one full rebuild finished
	dirty     bool // needs full rebuild (missed FS events / fingerprint)
	// dirtySeq bumps on MarkDirty / ApplyFSChange so a finishRebuild that raced
	// those mutations can keep dirty=true instead of trusting a stale walk.
	dirtySeq uint64
	bytes    int64
	lastUsed time.Time
	cancel   context.CancelFunc
}

// NewFileIndex returns an empty index for root.
func NewFileIndex(root string) *FileIndex {
	return &FileIndex{
		root:     root,
		byPath:   map[string]int{},
		lastUsed: time.Now(),
	}
}

// Snapshot is a consistent view of the index.
type Snapshot struct {
	Root      string
	Paths     []string
	Basenames []string
	Gen       uint64
	Ready     bool
	Building  bool
	Dirty     bool
	Bytes     int64
}

// Snapshot returns copies of the parallel arrays.
func (idx *FileIndex) Snapshot() Snapshot {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	return idx.snapshotLocked()
}

// StatusSnapshot returns readiness metadata without copying Paths/Basenames.
// Prefer this for bus publishes (services.go is readiness-only).
func (idx *FileIndex) StatusSnapshot() Snapshot {
	idx.mu.RLock()
	defer idx.mu.RUnlock()
	return Snapshot{
		Root:     idx.root,
		Gen:      idx.gen,
		Ready:    idx.ready,
		Building: idx.building,
		Dirty:    idx.dirty,
		Bytes:    idx.bytes,
	}
}

func (idx *FileIndex) snapshotLocked() Snapshot {
	paths := append([]string(nil), idx.paths...)
	bases := append([]string(nil), idx.basenames...)
	return Snapshot{
		Root:      idx.root,
		Paths:     paths,
		Basenames: bases,
		Gen:       idx.gen,
		Ready:     idx.ready,
		Building:  idx.building,
		Dirty:     idx.dirty,
		Bytes:     idx.bytes,
	}
}

func estimateIndexBytes(paths, basenames []string) int64 {
	var n int64
	for i, p := range paths {
		n += int64(len(p))
		if i < len(basenames) {
			n += int64(len(basenames[i]))
		}
		n += entryOverhead
	}
	return n
}

func (idx *FileIndex) recomputeBytesLocked() {
	idx.bytes = estimateIndexBytes(idx.paths, idx.basenames)
}

// ReplaceAll swaps in a complete path list (sorted) and bumps generation.
// Clears dirty unless an FS mutation races in before the swap completes.
func (idx *FileIndex) ReplaceAll(paths []string) {
	idx.mu.Lock()
	startSeq := idx.dirtySeq
	idx.dirty = false
	idx.mu.Unlock()
	idx.finishRebuild(paths, startSeq)
}

// finishRebuild installs a walk result. dirtySeq advances from MarkDirty /
// ApplyFSChange after startSeq was captured mean the walk may have missed
// those updates — keep dirty so focus/Ensure rebuilds. A dirty bit that was
// already set before the walk (the reason we rebuilt) is cleared.
func (idx *FileIndex) finishRebuild(paths []string, startSeq uint64) {
	sort.Strings(paths)
	bases := make([]string, len(paths))
	by := make(map[string]int, len(paths))
	for i, p := range paths {
		bases[i] = filepath.Base(p)
		by[p] = i
	}
	idx.mu.Lock()
	keepDirty := idx.dirtySeq != startSeq
	idx.paths = paths
	idx.basenames = bases
	idx.byPath = by
	idx.gen++
	idx.ready = true
	idx.building = false
	idx.dirty = keepDirty
	idx.recomputeBytesLocked()
	idx.lastUsed = time.Now()
	idx.mu.Unlock()
}

// MarkDirty notes that the snapshot may be incomplete (dropped FSEvents, etc.).
func (idx *FileIndex) MarkDirty() {
	idx.mu.Lock()
	idx.dirty = true
	idx.dirtySeq++
	idx.mu.Unlock()
}

// Upsert inserts or refreshes a repo-relative path (forward slashes).
func (idx *FileIndex) Upsert(rel string) {
	rel = normalizeRel(rel)
	if rel == "" || shouldSkipIndexed(rel) {
		return
	}
	base := filepath.Base(rel)
	idx.mu.Lock()
	defer idx.mu.Unlock()
	if i, ok := idx.byPath[rel]; ok {
		old := idx.basenames[i]
		idx.basenames[i] = base
		idx.bytes += int64(len(base) - len(old))
		idx.gen++
		idx.lastUsed = time.Now()
		return
	}
	idx.paths = append(idx.paths, rel)
	idx.basenames = append(idx.basenames, base)
	idx.byPath[rel] = len(idx.paths) - 1
	idx.bytes += int64(len(rel)+len(base)) + entryOverhead
	idx.gen++
	idx.lastUsed = time.Now()
}

// Remove deletes a repo-relative path if present.
func (idx *FileIndex) Remove(rel string) {
	rel = normalizeRel(rel)
	if rel == "" {
		return
	}
	idx.mu.Lock()
	defer idx.mu.Unlock()
	i, ok := idx.byPath[rel]
	if !ok {
		return
	}
	idx.bytes -= int64(len(idx.paths[i])+len(idx.basenames[i])) + entryOverhead
	if idx.bytes < 0 {
		idx.bytes = 0
	}
	last := len(idx.paths) - 1
	if i != last {
		swapPath := idx.paths[last]
		idx.paths[i] = swapPath
		idx.basenames[i] = idx.basenames[last]
		idx.byPath[swapPath] = i
	}
	idx.paths = idx.paths[:last]
	idx.basenames = idx.basenames[:last]
	delete(idx.byPath, rel)
	idx.gen++
	idx.lastUsed = time.Now()
}

// ApplyFSChange splices create/delete for a relative path by stating the file.
func (idx *FileIndex) ApplyFSChange(root, rel string) {
	rel = normalizeRel(rel)
	if rel == "" || shouldSkipIndexed(rel) {
		return
	}
	full := filepath.Join(root, filepath.FromSlash(rel))
	st, err := os.Lstat(full)
	if err != nil {
		idx.Remove(rel)
		idx.bumpDirtySeq()
		return
	}
	if st.IsDir() {
		return
	}
	if skipFile(filepath.Base(rel)) {
		idx.Remove(rel)
		idx.bumpDirtySeq()
		return
	}
	idx.Upsert(rel)
	idx.bumpDirtySeq()
}

func (idx *FileIndex) bumpDirtySeq() {
	idx.mu.Lock()
	idx.dirtySeq++
	idx.mu.Unlock()
}

// Rebuild lists files under root via ripgrep (or WalkDir fallback) asynchronously.
// resolveRg runs inside the build goroutine (may download) so RPC callers are not
// blocked. The build context is detached from ctx's cancellation — Wails cancels
// the ListFiles RPC context as soon as the method returns, which must not kill
// the walk. onUpdate may receive partial batches when streamPartials is true.
// force cancels an in-flight build; when false and already building, this is a no-op.
// extraGlobs / userSkip apply files.exclude (plus always-on .git/node_modules).
func (idx *FileIndex) Rebuild(ctx context.Context, resolveRg func(context.Context) string, onUpdate func(Snapshot), streamPartials, force bool, extraGlobs []string, userSkip excludeSet) {
	idx.mu.Lock()
	if idx.building && !force {
		idx.mu.Unlock()
		return
	}
	if idx.cancel != nil {
		idx.cancel()
	}
	// Detach from RPC/call cancellation; keep our own cancel for superseding builds.
	cctx, cancel := context.WithCancel(context.WithoutCancel(ctx))
	idx.cancel = cancel
	idx.building = true
	root := idx.root
	startSeq := idx.dirtySeq
	idx.lastUsed = time.Now()
	idx.mu.Unlock()

	go func() {
		defer cancel()
		rgPath := ""
		if resolveRg != nil {
			rgPath = resolveRg(cctx)
		}
		paths, err := listAllFiles(cctx, rgPath, root, extraGlobs, userSkip, func(partial []string) {
			if !streamPartials || onUpdate == nil {
				return
			}
			idx.mu.Lock()
			idx.mergePartial(partial, userSkip)
			// Do not copy Paths/Basenames here — every 500-file batch on a large
			// tree would allocate multi-MB snapshots and flood the UI bridge.
			// The hub debounces and Snapshot()s once at flush / final ready.
			snap := Snapshot{
				Root:     root,
				Gen:      idx.gen,
				Ready:    idx.ready,
				Building: true,
				Dirty:    idx.dirty,
				Bytes:    idx.bytes,
			}
			idx.mu.Unlock()
			onUpdate(snap)
		})
		if cctx.Err() != nil {
			idx.mu.Lock()
			idx.building = false
			idx.mu.Unlock()
			return
		}
		if err != nil {
			// Fall back to WalkDir when rg fails entirely and we got nothing.
			if len(paths) == 0 {
				paths, _ = ListFiles(cctx, root)
				paths = filterForIndex(paths, userSkip)
			}
		}
		idx.finishRebuild(paths, startSeq)
		if onUpdate != nil {
			onUpdate(idx.StatusSnapshot())
		}
	}()
}

// mergePartial appends unseen paths from a streaming batch (caller holds mu).
func (idx *FileIndex) mergePartial(partial []string, userSkip excludeSet) {
	for _, p := range partial {
		p = normalizeRel(p)
		if p == "" || !indexablePath(p, userSkip) {
			continue
		}
		if _, ok := idx.byPath[p]; ok {
			continue
		}
		base := filepath.Base(p)
		idx.paths = append(idx.paths, p)
		idx.basenames = append(idx.basenames, base)
		idx.byPath[p] = len(idx.paths) - 1
		idx.bytes += int64(len(p)+len(base)) + entryOverhead
	}
	idx.gen++
	idx.lastUsed = time.Now()
}

// listAllFiles prefers rg --files; falls back to WalkDir. batch may receive
// progressive chunks (nil to disable). extraGlobs are negated files.exclude
// patterns; userSkip post-filters WalkDir and guards rg mismatches.
func listAllFiles(ctx context.Context, rgPath, root string, extraGlobs []string, userSkip excludeSet, batch func([]string)) ([]string, error) {
	if rgPath != "" {
		var out []string
		var chunk []string
		err := rg.Files(ctx, rgPath, root, extraGlobs, func(rel string) error {
			if !indexablePath(rel, userSkip) {
				return nil
			}
			out = append(out, rel)
			if batch != nil {
				chunk = append(chunk, rel)
				if len(chunk) >= 500 {
					batch(chunk)
					chunk = nil
				}
			}
			return nil
		})
		if batch != nil && len(chunk) > 0 {
			batch(chunk)
		}
		if err == nil || len(out) > 0 {
			return out, err
		}
	}
	// WalkDir fallback (tests / rg missing).
	paths, err := ListFiles(ctx, root)
	return filterForIndex(paths, userSkip), err
}

func filterForIndex(paths []string, userSkip excludeSet) []string {
	if len(paths) == 0 {
		return paths
	}
	out := make([]string, 0, len(paths))
	for _, p := range paths {
		if indexablePath(p, userSkip) {
			out = append(out, p)
		}
	}
	return out
}

func indexablePath(rel string, userSkip excludeSet) bool {
	if shouldSkipIndexed(rel) || excluded(rel, userSkip) {
		return false
	}
	return true
}

func normalizeRel(rel string) string {
	rel = strings.TrimSpace(filepath.ToSlash(rel))
	rel = strings.TrimPrefix(rel, "./")
	if rel == "" || rel == "." || strings.HasPrefix(rel, "../") {
		return ""
	}
	return rel
}

// shouldSkipIndexed drops paths rg would exclude via DefaultGlobs, plus common
// binary/noise extensions. Hidden dirs (e.g. .github) stay — `rg --files -uu`.
func shouldSkipIndexed(rel string) bool {
	rel = "/" + rel + "/"
	if strings.Contains(rel, "/.git/") || strings.Contains(rel, "/node_modules/") {
		return true
	}
	base := filepath.Base(strings.TrimSuffix(rel, "/"))
	if base == "" || base == "." {
		return true
	}
	return skipFile(base)
}

// publishDebounce coalesces full-snapshot bus publishes after FS splices.
const publishDebounce = 300 * time.Millisecond

// IndexHub holds per-workspace file indexes under a resident byte budget.
// Cold workspaces are built on demand; the watched workspace is pinned against
// LRU eviction. Full rebuilds run on first Ensure and when dirty (focus /
// next Ensure) — not on every fingerprint tick.
//
// files.exclude (SetExclude) is baked into every listing so SearchFiles /
// @-mentions / Quick Open share the same explorer-visible corpus — no
// per-keystroke exclude filter.
type IndexHub struct {
	mu           sync.Mutex
	indexes      map[string]*FileIndex // workspaceID → index
	rgPath       string
	dataDir      string
	maxBytes     int64
	pinnedID     string // watched workspace; never LRU-evicted
	exclude      []string
	excludeSet   excludeSet
	rgGlobs      []string // negated exclude patterns for rg --glob
	publish      func(workspaceID string, snap Snapshot)
	pendingPub   map[string]struct{}
	publishTimer *time.Timer
}

// NewIndexHub constructs a hub with DefaultMaxIndexBytes. publish may be nil.
func NewIndexHub(dataDir string, publish func(workspaceID string, snap Snapshot)) *IndexHub {
	return &IndexHub{
		indexes:    map[string]*FileIndex{},
		dataDir:    dataDir,
		maxBytes:   DefaultMaxIndexBytes,
		publish:    publish,
		pendingPub: map[string]struct{}{},
	}
}

// SetExclude installs VS Code–style files.exclude patterns for all indexes.
// Always-on .git / node_modules stay in rg.DefaultGlobs. When the pattern set
// changes, warm indexes rebuild so search matches the explorer.
func (h *IndexHub) SetExclude(patterns []string) {
	next := normalizeExcludePatterns(patterns)
	h.mu.Lock()
	if samePatterns(h.exclude, next) {
		h.mu.Unlock()
		return
	}
	h.exclude = next
	h.excludeSet = compileExcludes(next)
	h.rgGlobs = excludeToRgGlobs(next)
	type warm struct {
		id  string
		idx *FileIndex
	}
	var rebuilds []warm
	for id, idx := range h.indexes {
		rebuilds = append(rebuilds, warm{id: id, idx: idx})
	}
	h.mu.Unlock()
	ctx := context.Background()
	for _, w := range rebuilds {
		w.idx.MarkDirty()
		h.rebuild(ctx, w.id, w.idx, true)
	}
}

// SetMaxBytes overrides the resident budget (tests / tuning). <=0 resets default.
func (h *IndexHub) SetMaxBytes(n int64) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if n <= 0 {
		n = DefaultMaxIndexBytes
	}
	h.maxBytes = n
	h.evictLocked()
}

// SetRgPath overrides the resolved ripgrep binary (tests).
func (h *IndexHub) SetRgPath(p string) {
	h.mu.Lock()
	h.rgPath = p
	h.mu.Unlock()
}

// SetPinned marks the watched workspace (empty clears). Pinned indexes are
// never LRU-evicted to free budget for colder trees.
func (h *IndexHub) SetPinned(workspaceID string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.pinnedID = workspaceID
	if workspaceID != "" {
		if idx := h.indexes[workspaceID]; idx != nil {
			idx.mu.Lock()
			idx.lastUsed = time.Now()
			idx.mu.Unlock()
		}
	}
	h.evictLocked()
}

// ensureRg resolves ripgrep under h.mu for the cached path. With a dataDir,
// rg.Ensure runs so a stale managed sidecar is upgraded (Locate alone would
// return the old binary and skip the version check). Ensure is a no-network
// hit when the sidecar already matches ReleaseVersion. Safe for concurrent
// RPC + bus callers; may download when invoked from the rebuild goroutine.
func (h *IndexHub) ensureRg(ctx context.Context) string {
	h.mu.Lock()
	if h.rgPath != "" {
		p := h.rgPath
		h.mu.Unlock()
		return p
	}
	dataDir := h.dataDir
	h.mu.Unlock()

	var p string
	if dataDir != "" {
		p, _ = rg.Ensure(ctx, dataDir)
	} else {
		p, _ = rg.Locate(dataDir)
	}

	h.mu.Lock()
	if h.rgPath == "" && p != "" {
		h.rgPath = p
	}
	out := h.rgPath
	h.mu.Unlock()
	if out != "" {
		return out
	}
	return p
}

// Ensure starts (or returns) the index for workspaceID at root.
// Rebuilds when missing/incomplete or dirty. Cold workspaces are not touched
// until this is called. An in-flight build is coalesced (not restarted).
func (h *IndexHub) Ensure(ctx context.Context, workspaceID, root string) Snapshot {
	idx := h.getOrCreate(workspaceID, root)
	h.touch(workspaceID)
	snap := idx.Snapshot()
	if snap.Root == root && snap.Building {
		return snap // coalesce — don't cancel a healthy in-flight walk
	}
	if snap.Ready && !snap.Dirty && snap.Root == root {
		return snap
	}
	h.rebuild(ctx, workspaceID, idx, false)
	return idx.Snapshot()
}

// Refresh forces a full rebuild (explicit RPC), canceling any in-flight walk.
func (h *IndexHub) Refresh(ctx context.Context, workspaceID, root string) Snapshot {
	idx := h.getOrCreate(workspaceID, root)
	h.touch(workspaceID)
	h.rebuild(ctx, workspaceID, idx, true)
	return idx.Snapshot()
}

// RefreshIfDirty rebuilds only when the index exists and is dirty/incomplete.
// No-op for cold workspaces (never Ensure'd) — they stay unloaded.
func (h *IndexHub) RefreshIfDirty(ctx context.Context, workspaceID, root string) Snapshot {
	h.mu.Lock()
	idx := h.indexes[workspaceID]
	h.mu.Unlock()
	if idx == nil {
		return Snapshot{Root: root}
	}
	if idx.root != root {
		idx = h.getOrCreate(workspaceID, root)
	}
	snap := idx.Snapshot()
	if snap.Building {
		h.touch(workspaceID)
		return snap
	}
	if snap.Ready && !snap.Dirty {
		h.touch(workspaceID)
		return snap
	}
	h.touch(workspaceID)
	h.rebuild(ctx, workspaceID, idx, false)
	return idx.Snapshot()
}

// Paths returns the current path snapshot (starts a build when empty/dirty).
func (h *IndexHub) Paths(ctx context.Context, workspaceID, root string) []string {
	return h.Ensure(ctx, workspaceID, root).Paths
}

// Search ensures the index then ranks paths for query.
//
// Ranks under RLock against the live slices — never Snapshot()-copies the corpus
// on the hot keystroke path. Exclude filtering happened at index build time.
func (h *IndexHub) Search(ctx context.Context, workspaceID, root, query string, limit int, filesOnly bool) domain.FileSearchResult {
	idx := h.getOrCreate(workspaceID, root)
	h.touch(workspaceID)

	idx.mu.RLock()
	building := idx.building
	ready := idx.ready
	dirty := idx.dirty
	rootOK := idx.root == root
	idx.mu.RUnlock()

	if rootOK && building {
		// coalesce — don't cancel a healthy in-flight walk
	} else if !(ready && !dirty && rootOK) {
		h.rebuild(ctx, workspaceID, idx, false)
	}

	idx.mu.RLock()
	hits := RankHits(ctx, idx.paths, idx.basenames, query, limit, filesOnly)
	out := domain.FileSearchResult{
		Hits:     hits,
		Building: idx.building,
		Ready:    idx.ready,
	}
	idx.mu.RUnlock()
	return out
}

// ApplyChanges splices path-level FS updates. rescan / empty paths mark dirty
// instead of immediately walking — focus or next Ensure rebuilds.
// No-op when the workspace has no warm index (cold). Snapshot publishes are
// debounced so bursty FSEvents don't flood the bridge with full copies.
func (h *IndexHub) ApplyChanges(ctx context.Context, workspaceID, root string, rels []string, rescan bool) {
	h.mu.Lock()
	idx := h.indexes[workspaceID]
	skip := h.excludeSet
	h.mu.Unlock()
	if idx == nil {
		return // cold — don't build just because the disk changed
	}
	if idx.root != root {
		idx = h.getOrCreate(workspaceID, root)
	}
	if rescan || len(rels) == 0 {
		idx.MarkDirty()
		h.touch(workspaceID)
		h.schedulePublish(workspaceID)
		return
	}
	for _, rel := range rels {
		rel = normalizeRel(rel)
		if rel == "" {
			continue
		}
		if !indexablePath(rel, skip) {
			idx.Remove(rel)
			continue
		}
		idx.ApplyFSChange(root, rel)
	}
	h.touch(workspaceID)
	h.mu.Lock()
	h.evictLocked()
	h.mu.Unlock()
	h.schedulePublish(workspaceID)
}

// MarkDirty marks a warm index dirty without rebuilding. No-op if cold.
func (h *IndexHub) MarkDirty(workspaceID string) {
	h.mu.Lock()
	idx := h.indexes[workspaceID]
	h.mu.Unlock()
	if idx == nil {
		return
	}
	idx.MarkDirty()
}

// Drop removes a workspace index (workspace closed / root changed).
func (h *IndexHub) Drop(workspaceID string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.dropLocked(workspaceID)
}

// TotalBytes returns the approximate resident size of all warm indexes.
func (h *IndexHub) TotalBytes() int64 {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.totalBytesLocked()
}

// Len returns how many workspace indexes are currently warm.
func (h *IndexHub) Len() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return len(h.indexes)
}

func (h *IndexHub) touch(workspaceID string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if idx := h.indexes[workspaceID]; idx != nil {
		idx.mu.Lock()
		idx.lastUsed = time.Now()
		idx.mu.Unlock()
	}
	h.evictLocked()
}

func (h *IndexHub) getOrCreate(workspaceID, root string) *FileIndex {
	h.mu.Lock()
	defer h.mu.Unlock()
	idx := h.indexes[workspaceID]
	if idx != nil && idx.root == root {
		return idx
	}
	if idx != nil {
		h.dropLocked(workspaceID)
	}
	idx = NewFileIndex(root)
	h.indexes[workspaceID] = idx
	h.evictLocked()
	return idx
}

func (h *IndexHub) dropLocked(workspaceID string) {
	if idx, ok := h.indexes[workspaceID]; ok {
		idx.mu.Lock()
		if idx.cancel != nil {
			idx.cancel()
		}
		idx.mu.Unlock()
		delete(h.indexes, workspaceID)
	}
}

func (h *IndexHub) totalBytesLocked() int64 {
	var n int64
	for _, idx := range h.indexes {
		idx.mu.RLock()
		n += idx.bytes
		idx.mu.RUnlock()
	}
	return n
}

// evictLocked drops LRU unpinned indexes until under maxBytes.
// Never evicts pinnedID. A single oversized pinned/active index is kept.
func (h *IndexHub) evictLocked() {
	if h.maxBytes <= 0 {
		return
	}
	for h.totalBytesLocked() > h.maxBytes && len(h.indexes) > 1 {
		var victim string
		var oldest time.Time
		first := true
		for id, idx := range h.indexes {
			if id == h.pinnedID {
				continue
			}
			idx.mu.RLock()
			lu := idx.lastUsed
			idx.mu.RUnlock()
			if first || lu.Before(oldest) {
				first = false
				oldest = lu
				victim = id
			}
		}
		if victim == "" {
			return // only pinned (or nothing evictable) remains
		}
		h.dropLocked(victim)
	}
}

func (h *IndexHub) rebuild(ctx context.Context, workspaceID string, idx *FileIndex, force bool) {
	h.mu.Lock()
	globs := append([]string(nil), h.rgGlobs...)
	skip := h.excludeSet
	h.mu.Unlock()
	idx.Rebuild(ctx, h.ensureRg, func(snap Snapshot) {
		h.mu.Lock()
		h.evictLocked()
		h.mu.Unlock()
		// Partials: debounce. Final ready: publish immediately (flush pending).
		if snap.Ready && !snap.Building {
			h.publishNow(workspaceID, snap)
			return
		}
		h.schedulePublish(workspaceID)
	}, true, force, globs, skip)
}

// publishNow emits snap for workspaceID immediately and flushes any other
// pending debounced publishes (so stopping the shared timer does not orphan them).
func (h *IndexHub) publishNow(workspaceID string, snap Snapshot) {
	h.mu.Lock()
	if h.publishTimer != nil {
		h.publishTimer.Stop()
		h.publishTimer = nil
	}
	if h.pendingPub != nil {
		delete(h.pendingPub, workspaceID)
	}
	pending := h.pendingPub
	h.pendingPub = map[string]struct{}{}
	pub := h.publish
	type item struct {
		id  string
		idx *FileIndex
	}
	var others []item
	for id := range pending {
		if idx := h.indexes[id]; idx != nil {
			others = append(others, item{id, idx})
		}
	}
	h.mu.Unlock()
	if pub == nil {
		return
	}
	pub(workspaceID, snap)
	for _, it := range others {
		pub(it.id, it.idx.StatusSnapshot())
	}
}

func (h *IndexHub) schedulePublish(workspaceID string) {
	if h.publish == nil {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.pendingPub == nil {
		h.pendingPub = map[string]struct{}{}
	}
	h.pendingPub[workspaceID] = struct{}{}
	if h.publishTimer != nil {
		h.publishTimer.Stop()
	}
	h.publishTimer = time.AfterFunc(publishDebounce, h.flushPublish)
}

func (h *IndexHub) flushPublish() {
	h.mu.Lock()
	pending := h.pendingPub
	h.pendingPub = map[string]struct{}{}
	h.publishTimer = nil
	pub := h.publish
	type item struct {
		id  string
		idx *FileIndex
	}
	var out []item
	for id := range pending {
		if idx := h.indexes[id]; idx != nil {
			out = append(out, item{id, idx})
		}
	}
	h.mu.Unlock()
	if pub == nil {
		return
	}
	for _, it := range out {
		pub(it.id, it.idx.StatusSnapshot())
	}
}
