-- 0001_v1_baseline — the original v1 schema (DECISIONS D5/D13).
--
-- Every statement is idempotent (CREATE ... IF NOT EXISTS) so this migration
-- applies cleanly to a database created before the migration runner existed:
-- re-running it over the pre-existing v1 tables is a harmless no-op that simply
-- stamps the database as schema version 1. The connection PRAGMAs (journal_mode,
-- foreign_keys) are NOT here — they are connection-level and run once per open in
-- runMigrations, not persisted in a migration.

CREATE TABLE IF NOT EXISTS projects (
	id          TEXT PRIMARY KEY,
	name        TEXT NOT NULL,
	repo_path   TEXT,
	base_branch TEXT,
	setup_cmd   TEXT,
	dev_cmd     TEXT,
	app_url     TEXT,
	extra_json  TEXT NOT NULL DEFAULT '{}',
	sort        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS loops (
	id        TEXT PRIMARY KEY,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS agents (
	loop_id   TEXT NOT NULL,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);

-- Per-loop activity feed, newest first: rows load back ORDER BY sort, so a
-- smaller sort means newer. Prepending is a single INSERT with a sort below
-- everything already there (see db.feedSort) rather than a rewrite of the loop's
-- rows. The index makes both that seed lookup and the delete-by-loop cascade cheap.
CREATE TABLE IF NOT EXISTS feed (
	loop_id   TEXT NOT NULL,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_feed_loop ON feed (loop_id, sort);

CREATE TABLE IF NOT EXISTS prs (
	id        TEXT PRIMARY KEY,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS escalations (
	loop_id   TEXT PRIMARY KEY,
	doc_json  TEXT NOT NULL
);

-- Per-loop Maestro chat transcript (PRD §8.5). doc_json is []MaestroChatMessage.
CREATE TABLE IF NOT EXISTS maestro_chat (
	loop_id   TEXT PRIMARY KEY,
	doc_json  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS inbox (
	id        TEXT PRIMARY KEY,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS policy (
	id        INTEGER PRIMARY KEY CHECK (id = 1),
	doc_json  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
	id        INTEGER PRIMARY KEY CHECK (id = 1),
	doc_json  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS resource (
	id        INTEGER PRIMARY KEY CHECK (id = 1),
	doc_json  TEXT NOT NULL
);

-- Append-only durable event log (PRD §6.5, §14; DECISIONS D13). Every
-- state-changing event the Bus carries is appended here so a reload reconstructs
-- the activity feed and history. NEVER updated in place — only INSERTed — so each
-- event has a stable identity and ordering (id autoincrement).
CREATE TABLE IF NOT EXISTS events (
	id           INTEGER PRIMARY KEY AUTOINCREMENT,
	loop_id      TEXT NOT NULL,
	agent_id     TEXT,
	type         TEXT NOT NULL,
	payload_json TEXT NOT NULL,
	ts           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_loop ON events (loop_id, id);

-- Reviewer findings (PRD §8.3 report_finding, §14). One row per finding; the
-- engine stamps loop_id/iteration/status (an agent can't forge them). Status
-- moves open→resolved across iterations as the Coder fixes them.
CREATE TABLE IF NOT EXISTS findings (
	id        TEXT PRIMARY KEY,
	loop_id   TEXT NOT NULL,
	doc_json  TEXT NOT NULL,
	sort      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_findings_loop ON findings (loop_id, sort);

-- Observatory analysis runs (ADE metrics/impact/drift). One JSON doc per run;
-- latest-per-project is derived in memory.
CREATE TABLE IF NOT EXISTS analysis (
	id          TEXT PRIMARY KEY,
	project_id  TEXT NOT NULL,
	loop_id     TEXT,
	doc_json    TEXT NOT NULL,
	ts          INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_analysis_project ON analysis (project_id, ts);
