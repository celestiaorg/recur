-- 0015_scheduled_prompts: schedules that run a prompt in a fresh workspace.
-- Fully normalized: one row per schedule, one row per model flag, one row per
-- occurrence. Occurrences are claimed by (schedule_id, due_at) so a repeated
-- timer tick or a restart cannot launch the same due time twice.

CREATE TABLE scheduled_prompt (
	id          TEXT PRIMARY KEY,
	project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
	name        TEXT NOT NULL,
	prompt      TEXT NOT NULL,
	base_branch TEXT NOT NULL,
	mode        TEXT NOT NULL CHECK (mode IN ('ask', 'plan', 'agent', 'review', 'loop')),
	model       TEXT NOT NULL,
	simplify    INTEGER NOT NULL DEFAULT 1 CHECK (simplify IN (0, 1)),
	loop_id     TEXT NOT NULL DEFAULT '',
	policy_id   TEXT NOT NULL DEFAULT '',
	-- once: run_date + run_time. cron: cron expression. daily/weekdays: legacy
	-- wall-clock cadences kept readable until edited into cron.
	cadence     TEXT NOT NULL CHECK (cadence IN ('once', 'cron', 'daily', 'weekdays')),
	cron        TEXT NOT NULL DEFAULT '',
	run_date    TEXT NOT NULL DEFAULT '',
	run_time    TEXT NOT NULL DEFAULT '',
	time_zone   TEXT NOT NULL,
	paused      INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1)),
	-- All times are ISO 8601 UTC text with milliseconds (2026-09-09T13:05:00.000Z):
	-- readable in a sqlite shell and sorts lexicographically. '' = no next run.
	next_run_at TEXT NOT NULL DEFAULT '',
	created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
	updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX scheduled_prompt_project ON scheduled_prompt(project_id);
CREATE INDEX scheduled_prompt_due ON scheduled_prompt(next_run_at) WHERE paused = 0 AND next_run_at <> '';

CREATE TABLE scheduled_prompt_flag (
	schedule_id TEXT NOT NULL REFERENCES scheduled_prompt(id) ON DELETE CASCADE,
	key         TEXT NOT NULL,
	value       TEXT NOT NULL,
	PRIMARY KEY (schedule_id, key)
);

CREATE TABLE scheduled_prompt_run (
	id           TEXT PRIMARY KEY,
	schedule_id  TEXT NOT NULL REFERENCES scheduled_prompt(id) ON DELETE CASCADE,
	due_at       TEXT NOT NULL,
	workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
	status       TEXT NOT NULL CHECK (status IN ('running', 'waiting', 'finished', 'failed', 'stopped', 'missed', 'skipped', 'interrupted')),
	detail       TEXT NOT NULL DEFAULT '',
	created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
	UNIQUE (schedule_id, due_at)
);
CREATE INDEX scheduled_prompt_run_schedule ON scheduled_prompt_run(schedule_id, created_at);
CREATE INDEX scheduled_prompt_run_active ON scheduled_prompt_run(created_at)
	WHERE status IN ('running', 'waiting', 'interrupted');
