-- Sidebar project groups: named collapsible sections that nest projects.
-- projects.group_id is empty for ungrouped projects (shown after all groups).

CREATE TABLE IF NOT EXISTS project_groups (
	id         TEXT PRIMARY KEY,
	name       TEXT NOT NULL,
	open       INTEGER NOT NULL DEFAULT 1,
	sort       INTEGER NOT NULL DEFAULT 0,
	created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
	updated_at TEXT NOT NULL DEFAULT ''
);

ALTER TABLE projects ADD COLUMN group_id TEXT NOT NULL DEFAULT '';
