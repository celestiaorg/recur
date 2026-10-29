-- Cached GitHub branch rules: whether a pull request is required to land on a
-- branch. Rarely changes; Recur re-checks on a long TTL rather than every land.

CREATE TABLE github_branch_rules (
	project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
	repo       TEXT NOT NULL,
	branch     TEXT NOT NULL,
	require_pr INTEGER NOT NULL CHECK (require_pr IN (0, 1)),
	checked_at INTEGER NOT NULL,
	PRIMARY KEY (project_id, repo, branch)
);
