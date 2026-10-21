package store

import (
	"os"
	"path/filepath"
)

// dataDirEnv overrides the managed root so tests (and power users) can point
// Recur at a throwaway directory instead of the real app-support location.
const dataDirEnv = "RECUR_DATA_DIR"

// worktreesDirEnv overrides the git-worktrees parent. When unset, production uses
// ~/.recur/worktrees (kept outside the data dir so a data-dir copy for dogfood
// does not duplicate worktrees). Tests that set $RECUR_DATA_DIR keep worktrees
// under that temp root unless $RECUR_WORKTREES_DIR is also set.
const worktreesDirEnv = "RECUR_WORKTREES_DIR"

// DataDir returns Recur's managed root, creating it if needed (PRD §14):
//
//	~/Library/Application Support/Recur/   (macOS; via os.UserConfigDir)
//
// $RECUR_DATA_DIR overrides it entirely. The root holds recur.db, per-workspace
// data under workspaces/<id>/, window.json, shell.json, schedule-view.json, and the cursor-bridge cache. Git
// worktrees live separately under WorktreesDir.
func DataDir() (string, error) {
	root := os.Getenv(dataDirEnv)
	if root == "" {
		cfg, err := os.UserConfigDir()
		if err != nil {
			return "", err
		}
		root = filepath.Join(cfg, "Recur")
	}
	if err := os.MkdirAll(root, 0o755); err != nil {
		return "", err
	}
	return root, nil
}

// DBPath returns the SQLite database path under the managed root.
func DBPath(root string) string { return filepath.Join(root, "recur.db") }

// OpenSockPath is the unix socket the desktop app listens on so `recur .`
// can hand a path to the running instance.
func OpenSockPath() (string, error) {
	root, err := DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, "open.sock"), nil
}

// OpenFilesDir is the empty working-copy root for the singleton "Files"
// project that hosts homeless (non-project) editor tabs.
func OpenFilesDir() (string, error) {
	root, err := DataDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(root, "open-files")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// WorktreesDir returns the per-workspace git worktree parent, creating it.
//
// Resolution order:
//  1. $RECUR_WORKTREES_DIR when set
//  2. $RECUR_DATA_DIR/worktrees when a data-dir override is active (keeps tests
//     and alternate instances self-contained)
//  3. ~/.recur/worktrees (production default); per-workspace checkouts live at
//     ~/.recur/worktrees/<project-slug>/<workspace-slug>
//
// On first use of the production default, any leftover worktrees under the
// legacy DataDir()/worktrees location are moved across (best-effort).
func WorktreesDir() (string, error) {
	if v := os.Getenv(worktreesDirEnv); v != "" {
		if err := os.MkdirAll(v, 0o755); err != nil {
			return "", err
		}
		return v, nil
	}
	if data := os.Getenv(dataDirEnv); data != "" {
		dir := filepath.Join(data, "worktrees")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return "", err
		}
		return dir, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(home, ".recur", "worktrees")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	migrateLegacyWorktrees(dir)
	return dir, nil
}

// migrateLegacyWorktrees moves entries from the pre-split
// ~/Library/Application Support/Recur/worktrees into ~/.recur/worktrees when
// the new root is empty. Best-effort: failures leave the legacy tree alone.
func migrateLegacyWorktrees(newRoot string) {
	cfg, err := os.UserConfigDir()
	if err != nil {
		return
	}
	legacy := filepath.Join(cfg, "Recur", "worktrees")
	if legacy == newRoot {
		return
	}
	entries, err := os.ReadDir(legacy)
	if err != nil || len(entries) == 0 {
		return
	}
	newEntries, err := os.ReadDir(newRoot)
	if err != nil || len(newEntries) > 0 {
		return // already populated — don't merge/clobber
	}
	for _, e := range entries {
		src := filepath.Join(legacy, e.Name())
		dst := filepath.Join(newRoot, e.Name())
		if err := os.Rename(src, dst); err != nil {
			// Cross-device rename can fail; fall back to leave legacy in place.
			return
		}
	}
	_ = os.Remove(legacy) // remove only if empty
}

// WorkspaceDir returns the per-workspace managed data directory:
//
//	<DataDir>/workspaces/<workspaceID>/
//
// creating it if needed. Agent artifacts, chat attachments, and QA screenshots
// live under this tree (not inside the git worktree).
func WorkspaceDir(root, workspaceID string) (string, error) {
	if workspaceID == "" {
		return "", os.ErrInvalid
	}
	dir := filepath.Join(root, "workspaces", workspaceID)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// ArtifactsDir returns the agent-created artifacts parent for a workspace,
// creating workspaces/<workspaceID>/artifacts/.
func ArtifactsDir(root, workspaceID string) (string, error) {
	ws, err := WorkspaceDir(root, workspaceID)
	if err != nil {
		return "", err
	}
	dir := filepath.Join(ws, "artifacts")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// AttachmentsDir returns the chat-paste attachment parent for a workspace,
// creating workspaces/<workspaceID>/attachments/.
func AttachmentsDir(root, workspaceID string) (string, error) {
	ws, err := WorkspaceDir(root, workspaceID)
	if err != nil {
		return "", err
	}
	dir := filepath.Join(ws, "attachments")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// ScreenshotsDir returns the QA evidence screenshots parent for a workspace,
// creating workspaces/<workspaceID>/screenshots/ (PRD §10).
func ScreenshotsDir(root, workspaceID string) (string, error) {
	ws, err := WorkspaceDir(root, workspaceID)
	if err != nil {
		return "", err
	}
	dir := filepath.Join(ws, "screenshots")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// AppChatDir is the provider cwd for the app-scoped Ask Recur coordinator.
// It is not a git worktree — the chat is read-only on Recur
// state and must not inherit a project checkout as cmd.Dir.
func AppChatDir() (string, error) {
	root, err := DataDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(root, "app-chat")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// RecurSkillsDir returns the seeded Recur-owned skills root. Alternate data
// profiles keep it under that root, so isolation does not require replacing HOME
// and hiding the user's desktop/browser preferences.
func RecurSkillsDir() (string, error) {
	if root := os.Getenv(dataDirEnv); root != "" {
		dir := filepath.Join(root, "recur-skills")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return "", err
		}
		return dir, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(home, ".recur", "recur-skills")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// LogsDir returns the app log directory under the managed root
// (<dataDir>/logs/), creating it if needed. Recur process logs (recur.log)
// and Cursor bridge stderr land here.
func LogsDir(root string) (string, error) {
	dir := filepath.Join(root, "logs")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// BundlesDir returns the default (read-only shipped) bundles root under the
// managed data dir (<DataDir>/Bundles/), creating it if needed.
func BundlesDir() (string, error) {
	root, err := DataDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(root, "Bundles")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// UserBundlesDir returns the user-writable bundles root (~/.recur/bundles),
// creating it if needed. User bundles override defaults by command uuid.
func UserBundlesDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(home, ".recur", "bundles")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}

// BundlesCacheDir returns the compiled typescript/js macro cache under the
// managed data dir (<DataDir>/BundleCache/), creating it if needed.
func BundlesCacheDir() (string, error) {
	root, err := DataDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(root, "BundleCache")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	return dir, nil
}
