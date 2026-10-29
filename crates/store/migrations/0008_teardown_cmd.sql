-- Per-project teardown command: runs once when a workspace's worktree is
-- archived or deleted (symmetric with setup_cmd on worktree create).
ALTER TABLE projects ADD COLUMN teardown_cmd TEXT NOT NULL DEFAULT '';
