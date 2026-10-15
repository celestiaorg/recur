package engine

import (
	"context"
	"crypto/sha256"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"

	gitpkg "recur/internal/git"
)

// Worktree watcher: the ACTIVE workspace's worktree is watched for changes so the
// Observatory (file explorer, source control) tracks the disk in near-real-time —
// no matter who changed it: an agent turn, the user's editor, a terminal, a git
// command. One workspace is watched at a time (the one on screen; the frontend
// arms it and disarms on leave), and one bus event (workspace.fs_changed) is the
// single refresh trigger every surface reacts to. The frontend responds with a
// debounced SHALLOW re-list of what's on screen (expanded dirs, changed files,
// git status) — the VS Code model — never a full-tree rebuild per event.
//
// Fast path on macOS: FSEvents directly (fswatch_darwin.go): ONE recursive
// kernel stream per worktree root (not kqueue's fd-per-directory), with kernel-side coalescing via the stream's Latency, and
// the MustScanSubDirs flag honored as a forced refresh when the kernel dropped
// events. NoDefer delivers the first event after idle immediately (interactive
// explorer) and still coalesces bursts into one batch per Latency window. The
// stream is armed on the symlink-resolved root (/var → /private/var) so
// ItemModified events are not dropped. Events under .git/ are ignored as
// noise (commits/branch switches are covered by the fingerprint backstop).
// Recur's own `git status` polls set GIT_OPTIONAL_LOCKS=0 so they do not
// rewrite .git/index. Backstop: a slow `git status --porcelain`
// fingerprint poll catches what the filtered stream can't see — .git-only
// changes (commits, branch switches) — and doubles as the worktree-deleted
// detector (Stat). Non-git working copies skip the fingerprint entirely;
// FSEvents is their only refresh signal. Stream-start failure is logged and
// degrades to that backstop (a nil channel never receives). Other platforms use
// fsnotify (fswatch_fsnotify.go) behind the same startFSWatch contract.

// fsEventLatency is the coalescing window: bursts (an agent writing many files,
// npm install) arrive as one batch per window. Isolated events after idle skip
// this wait (FSEvents NoDefer; the fsnotify batcher flushes the first event).
var fsEventLatency = 400 * time.Millisecond

// fsEvent is one watcher notification: an absolute, symlink-resolved path, or
// Rescan when the platform dropped events and the whole tree must be re-listed.
type fsEvent struct {
	Path   string
	Rescan bool
}

// fsPollBackstop is the slow fingerprint-poll cadence. Vars so tests can shrink.
var fsPollBackstop = 7 * time.Second

// WatchedWorkspace returns the workspace id currently armed for FSEvents, or "".
func (e *Engine) WatchedWorkspace() string {
	e.watchMu.Lock()
	defer e.watchMu.Unlock()
	return e.watchedID
}

// WatchWorkspace makes id the single watched workspace (replacing any previous
// watch); an empty id stops watching. Safe to call repeatedly with the same id —
// the watch is restarted, which is harmless (the stream starts at "now").
func (e *Engine) WatchWorkspace(id string) {
	e.watchMu.Lock()
	defer e.watchMu.Unlock()
	if e.watchCancel != nil {
		e.watchCancel()
		e.watchCancel = nil
	}
	e.watchedID = id
	// Tell the file-index hub which workspace to pin (and to unpin on leave).
	e.Bus.Publish(Event{Name: "workspace.watched", Data: map[string]any{
		"workspaceId": id,
	}})
	if id == "" {
		// Leaving the Observatory — no chat tab is on screen anymore.
		e.SetWorkspaceChatFocus("", "")
		return
	}
	// Switching (or re-watching) a workspace: drop prior focus so a background
	// finish on the previous tab can raise unseen until the UI re-focuses.
	e.chatFocusMu.Lock()
	if e.chatFocus.workspaceID != id {
		e.chatFocus.workspaceID = ""
		e.chatFocus.code = ""
	}
	e.chatFocusMu.Unlock()
	ws, ok := e.Store.Workspace(id)
	if !ok || ws.WorktreePath == "" {
		return
	}
	ctx, cancel := context.WithCancel(context.Background())
	e.watchCancel = cancel
	go e.watchLoop(ctx, id, ws.WorktreePath)
}

// watchLoop pumps the FSEvents stream + the fingerprint backstop into
// workspace.fs_changed publishes, until cancelled or the worktree vanishes.
func (e *Engine) watchLoop(ctx context.Context, id, worktree string) {
	// FSEvents delivers symlink-RESOLVED paths (/var → /private/var). Watch AND
	// filter on the resolved root: arming the unresolved /var/folders/... path
	// silently drops ItemModified events (creates still arrive), so open-file
	// live refresh never fired for edits — and non-git working copies have no
	// git-status fingerprint to catch them on the backstop.
	watchRoot := worktree
	if resolved, err := filepath.EvalSymlinks(worktree); err == nil {
		watchRoot = resolved
	}
	ignoreRoot := watchRoot

	batches, stopWatch, err := startFSWatch(watchRoot)
	if err != nil {
		log.Printf("recur: fs watch on %s failed (%v) — falling back to %s fingerprint poll",
			watchRoot, err, fsPollBackstop)
	} else {
		defer stopWatch()
	}
	// Classify once: a sticky IsRepo avoids flapping the fingerprint through the
	// "nogit" sentinel on a transient `git status` failure (timeout / spawn blip),
	// which would otherwise publish two full-rescan events per blip.
	isGit := gitpkg.IsRepo(worktree)

	publish := func(rels []string, rescan bool) {
		data := map[string]any{
			"workspaceId": id,
			"rescan":      rescan,
		}
		if len(rels) > 0 {
			data["paths"] = rels
		}
		e.Bus.Publish(Event{Name: "workspace.fs_changed", Data: data})
	}

	var last [32]byte
	baselined := false
	poll := time.NewTicker(fsPollBackstop)
	defer poll.Stop()

	for {
		select {
		case <-ctx.Done():
			return

		case batch := <-batches:
			// One kernel-coalesced batch per Latency window; refresh once if it
			// contains anything relevant. MustScanSubDirs means the kernel DROPPED
			// events for that subtree — refresh unconditionally. A branch
			// switch/checkout only rewrites .git/HEAD, which the rest of the .git
			// filter would swallow, so surface it explicitly (fsIsHeadChange) — that
			// keeps the branch label live on file-granularity streams (real Macs)
			// without waiting for the fingerprint backstop.
			refresh, headChanged, rescan := false, false, false
			var rels []string
			seen := map[string]bool{}
			for _, ev := range batch {
				p := ev.Path
				switch {
				case ev.Rescan:
					// The kernel DROPPED events for this subtree — a churny checkout
					// or rebase is exactly when that happens, so the .git/HEAD rewrite
					// may be among the dropped events. Sync the branch too, not just
					// the panels, or the label waits for the fingerprint backstop.
					refresh, headChanged, rescan = true, true, true
				case fsIsHeadChange(ignoreRoot, p):
					refresh, headChanged = true, true
				case !fsIgnored(ignoreRoot, p):
					refresh = true
					if rel, err := filepath.Rel(ignoreRoot, p); err == nil {
						rel = filepath.ToSlash(rel)
						if rel != "." && !seen[rel] {
							seen[rel] = true
							rels = append(rels, rel)
						}
					}
				}
			}
			if headChanged {
				// Sync the cached branch before panels refresh: a read-only
				// `git branch --show-current` that no-ops when unchanged (never
				// writes .git, so it can't feed the refresh loop).
				e.syncWorkspaceBranch(id)
			}
			if refresh {
				// Sidebar +N/−M (debounced) — same trigger as Observatory panels.
				e.scheduleWorkspaceDiffSync(id)
				publish(rels, rescan)
			}

		case <-poll.C:
			if st, err := os.Stat(worktree); err != nil || !st.IsDir() {
				return // worktree gone (workspace deleted/discarded) — stop quietly
			}
			if !isGit {
				// Plain folder: FSEvents covers content; Stat above is the
				// deleted-worktree detector. No fingerprint to flap.
				continue
			}
			fp, ok := worktreeFingerprint(ctx, worktree, e.workspaceBaseName(id))
			if !ok {
				// Transient git failure (timeout, spawn pressure) — keep the
				// last baseline; do not publish a spurious full rescan.
				continue
			}
			if baselined && fp != last {
				// Branch renames don't touch the working tree; sync the cached
				// Workspace.Branch (and Local PR.Branch) before panels refresh.
				e.syncWorkspaceBranch(id)
				e.scheduleWorkspaceDiffSync(id)
				publish(nil, true) // fingerprint-only: force file-index rescan
			}
			last = fp
			baselined = true
		}
	}
}

// fsIgnored filters watch-stream noise: everything under .git (also breaks the
// git-status feedback loop — commits/branch switches are covered by the
// fingerprint backstop), dependency/build churn, and editor droppings.
func fsIgnored(root, path string) bool {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return false
	}
	rel = filepath.ToSlash(rel)
	if rel == ".git" || strings.HasPrefix(rel, ".git/") {
		return true
	}
	if strings.HasSuffix(rel, "/.DS_Store") || rel == ".DS_Store" {
		return true
	}
	return strings.Contains("/"+rel+"/", "/node_modules/")
}

// fsIsHeadChange reports whether path is the worktree's .git/HEAD — the file a
// branch switch/checkout rewrites. It is surfaced on the fast stream (the rest of
// .git is filtered) so the titlebar/sidebar branch updates within the FSEvents
// latency window rather than waiting for the fingerprint backstop. Unlike
// .git/index (rewritten by git add/commit — Recur's status polls set
// GIT_OPTIONAL_LOCKS=0 so they do not), HEAD is untouched by status, so
// surfacing it cannot create a refresh feedback loop.
// Linked worktrees keep HEAD outside the tree (.git is a file), so this only ever
// matches the main working copy — exactly where a live branch label is expected.
func fsIsHeadChange(root, path string) bool {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return false
	}
	return filepath.ToSlash(rel) == ".git/HEAD"
}

// workspaceBaseName is the merge-base ref the titlebar Update button compares
// against (workspace BaseBranch, else the project's). Empty for a missing
// workspace — the fingerprint then hashes only HEAD / @{upstream}.
func (e *Engine) workspaceBaseName(id string) string {
	ws, ok := e.Store.Workspace(id)
	if !ok {
		return ""
	}
	base := strings.TrimSpace(ws.BaseBranch)
	if base == "" {
		if p, pok := e.Store.Project(ws.ProjectID); pok {
			base = strings.TrimSpace(p.BaseBranch)
		}
	}
	return base
}

// gitRevParse returns the SHA for rev, or "" if it is missing. CombinedOutput
// on failure includes git's stderr (e.g. "fatal: no upstream configured"); that
// must never enter the fingerprint or a no-upstream worktree hashes error text
// and a later timeout flaps two full rescans.
func gitRevParse(ctx context.Context, dir, rev string) string {
	out, err := runGit(ctx, dir, "rev-parse", "--verify", "--quiet", rev)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

func gitOutOrEmpty(ctx context.Context, dir string, args ...string) string {
	out, err := runGit(ctx, dir, args...)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(out)
}

// worktreeFingerprint hashes the worktree's `git status --porcelain -z`, HEAD,
// the current branch name, @{upstream}, and the workspace merge-base tip.
// HEAD/branch catch commits, checkouts, and renames. @{upstream} catches
// `git pull` of the current branch. The base tip plus Manager.CompareRef
// (the same ref WorkspaceLandState uses) catch `git fetch` that advances the
// merge base while this worktree sits on an unpublished feature branch — the
// titlebar "Update with main" case, including slashy base names and remotes
// not named origin. Missing optional refs (unborn HEAD, no upstream, unknown
// base) hash as empty; they must not leak git stderr or disable the backstop.
//
// ok=false means git failed — the caller keeps its previous baseline (soft fail)
// instead of flapping through a sentinel. Directory liveness is checked by the
// caller via Stat; non-git worktrees never call this.
func worktreeFingerprint(ctx context.Context, worktree, base string) ([32]byte, bool) {
	cctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	status, err := runGit(cctx, worktree, "status", "--porcelain", "-z")
	if err != nil {
		return [32]byte{}, false
	}
	head := gitRevParse(cctx, worktree, "HEAD")
	branch := gitOutOrEmpty(cctx, worktree, "branch", "--show-current")
	upstream := gitRevParse(cctx, worktree, "@{upstream}")
	base = strings.TrimSpace(base)
	baseTip, compareTip := "", ""
	if base != "" {
		baseTip = gitRevParse(cctx, worktree, base)
		compare := gitpkg.NewManager(worktree, "").CompareRef(cctx, base)
		if compare != "" && compare != base {
			compareTip = gitRevParse(cctx, worktree, compare)
		}
	}
	return sha256.Sum256([]byte(status + "\x00" + head + "\x00" + branch + "\x00" + upstream + "\x00" + baseTip + "\x00" + compareTip)), true
}
