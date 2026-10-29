-- 0002_sessions — the v2 session/message/content model.
--
-- A lossless, provider-neutral transcript store. The session_message join owns
-- ordering so forking is a cheap row copy; message_content holds typed blocks
-- (open type set, no CHECK) so unknown types round-trip; provider_event mirrors
-- verbatim provider lines as forward-compat insurance.

CREATE TABLE IF NOT EXISTS session (
	id                   TEXT PRIMARY KEY,
	workspace_id         TEXT,
	loop_id              TEXT,
	kind                 TEXT NOT NULL,
	parent_session_id    TEXT,
	fork_message_id      TEXT,
	context_mode         TEXT NOT NULL,
	provider             TEXT NOT NULL,
	provider_session_ref TEXT,
	config_json          TEXT NOT NULL DEFAULT '{}',
	active_context_id    TEXT,
	status               TEXT NOT NULL DEFAULT 'idle',
	created_at           INTEGER NOT NULL,
	updated_at           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_workspace ON session (workspace_id, updated_at);
CREATE INDEX IF NOT EXISTS idx_session_loop ON session (loop_id) WHERE loop_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS message (
	id                    TEXT PRIMARY KEY,
	origin_session_id     TEXT NOT NULL,
	role                  TEXT NOT NULL,
	model_id              TEXT,
	provider              TEXT,
	stream_status         TEXT NOT NULL DEFAULT 'pending',
	started_at            INTEGER,
	completed_at          INTEGER,
	input_tokens          INTEGER,
	output_tokens         INTEGER,
	cache_creation_tokens INTEGER,
	cache_read_tokens     INTEGER,
	cost_usd              REAL,
	created_at            INTEGER NOT NULL
);

-- M:N join. ORDERING LIVES HERE (not on message) — this is what makes copy-free
-- forking implementable.
CREATE TABLE IF NOT EXISTS session_message (
	session_id  TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
	message_id  TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
	idx         INTEGER NOT NULL,
	PRIMARY KEY (session_id, message_id)
);
CREATE INDEX IF NOT EXISTS idx_session_message ON session_message (session_id, idx);

CREATE TABLE IF NOT EXISTS message_content (
	id           TEXT PRIMARY KEY,
	message_id   TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
	idx          INTEGER NOT NULL,
	type         TEXT NOT NULL,
	content_json TEXT NOT NULL,
	created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_content_msg ON message_content (message_id, idx);
-- Idempotency backstop: at most one result per tool call per message (tim-db's
-- unique partial-index trick; json_extract works in SQLite).
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_result_per_call
	ON message_content (message_id, json_extract(content_json, '$.tool_use_id'))
	WHERE type = 'tool_result';

-- Compaction (app mode). Full history is untouched; this is a summary + watermark.
CREATE TABLE IF NOT EXISTS session_context (
	id                        TEXT PRIMARY KEY,
	session_id                TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
	summary                   TEXT NOT NULL,
	last_compacted_message_id TEXT NOT NULL,
	created_at                INTEGER NOT NULL
);

-- Raw mirror (Recur-specific; neither prior art has this).
CREATE TABLE IF NOT EXISTS provider_event (
	id         INTEGER PRIMARY KEY AUTOINCREMENT,
	session_id TEXT NOT NULL,
	message_id TEXT,
	seq        INTEGER NOT NULL,
	payload    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_provider_event ON provider_event (session_id, seq);
