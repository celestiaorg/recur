-- 0022_watch_schedules: a watch cadence whose command's stdout lines start
-- runs. SQLite cannot alter a CHECK, so scheduled_prompt is rebuilt. Its
-- children are rebuilt beside it first: with foreign_keys on, dropping the
-- old parent would otherwise cascade-delete their rows.

CREATE TABLE scheduled_prompt_new (
	id            TEXT PRIMARY KEY,
	project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
	name          TEXT NOT NULL,
	prompt        TEXT NOT NULL,
	base_branch   TEXT NOT NULL,
	mode          TEXT NOT NULL CHECK (mode IN ('ask', 'plan', 'agent', 'review', 'loop')),
	model         TEXT NOT NULL,
	simplify      INTEGER NOT NULL DEFAULT 1 CHECK (simplify IN (0, 1)),
	loop_id       TEXT NOT NULL DEFAULT '',
	policy_id     TEXT NOT NULL DEFAULT '',
	-- once: run_date + run_time. cron: cron expression. watch: watch_command.
	-- daily/weekdays: legacy wall-clock cadences kept readable until edited into cron.
	cadence       TEXT NOT NULL CHECK (cadence IN ('once', 'cron', 'daily', 'weekdays', 'watch')),
	cron          TEXT NOT NULL DEFAULT '',
	watch_command TEXT NOT NULL DEFAULT '',
	run_date      TEXT NOT NULL DEFAULT '',
	run_time      TEXT NOT NULL DEFAULT '',
	time_zone     TEXT NOT NULL,
	paused        INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1)),
	next_run_at   TEXT NOT NULL DEFAULT '',
	created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
	updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT INTO scheduled_prompt_new (
	id, project_id, name, prompt, base_branch, mode, model, simplify, loop_id, policy_id,
	cadence, cron, run_date, run_time, time_zone, paused, next_run_at, created_at, updated_at
)
SELECT
	id, project_id, name, prompt, base_branch, mode, model, simplify, loop_id, policy_id,
	cadence, cron, run_date, run_time, time_zone, paused, next_run_at, created_at, updated_at
FROM scheduled_prompt ORDER BY rowid;

CREATE TABLE scheduled_prompt_flag_new (
	schedule_id TEXT NOT NULL REFERENCES scheduled_prompt_new(id) ON DELETE CASCADE,
	key         TEXT NOT NULL,
	value       TEXT NOT NULL,
	PRIMARY KEY (schedule_id, key)
);
INSERT INTO scheduled_prompt_flag_new SELECT schedule_id, key, value FROM scheduled_prompt_flag;

CREATE TABLE scheduled_prompt_run_new (
	id           TEXT PRIMARY KEY,
	schedule_id  TEXT NOT NULL REFERENCES scheduled_prompt_new(id) ON DELETE CASCADE,
	due_at       TEXT NOT NULL,
	workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
	status       TEXT NOT NULL CHECK (status IN ('running', 'waiting', 'finished', 'failed', 'stopped', 'missed', 'skipped', 'interrupted')),
	detail       TEXT NOT NULL DEFAULT '',
	created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
	UNIQUE (schedule_id, due_at)
);
INSERT INTO scheduled_prompt_run_new (id, schedule_id, due_at, workspace_id, status, detail, created_at)
SELECT id, schedule_id, due_at, workspace_id, status, detail, created_at
FROM scheduled_prompt_run ORDER BY rowid;

DROP TABLE scheduled_prompt_run;
DROP TABLE scheduled_prompt_flag;
DROP TABLE scheduled_prompt;
ALTER TABLE scheduled_prompt_new RENAME TO scheduled_prompt;
ALTER TABLE scheduled_prompt_flag_new RENAME TO scheduled_prompt_flag;
ALTER TABLE scheduled_prompt_run_new RENAME TO scheduled_prompt_run;

CREATE INDEX scheduled_prompt_project ON scheduled_prompt(project_id);
CREATE INDEX scheduled_prompt_due ON scheduled_prompt(next_run_at) WHERE paused = 0 AND next_run_at <> '';
CREATE INDEX scheduled_prompt_run_schedule ON scheduled_prompt_run(schedule_id, created_at);
CREATE INDEX scheduled_prompt_run_active ON scheduled_prompt_run(created_at)
	WHERE status IN ('running', 'waiting', 'interrupted');
