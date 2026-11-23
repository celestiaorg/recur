-- 0020_insights: rows and indexes the Insights report aggregates by time
-- range, so it never scans transcript JSON at query time.
--
-- tool_call: one row per tool_use block, closed by its tool_result.
-- tool_use_id is the tool_use JSON id (what a tool_result's tool_use_id
-- names), not the message_content row id. label is the program or tool group
-- ("cargo test", "Read / Edit files"). Backfills call recur_tool_label,
-- recur_tool_no_match and recur_uuid7, Rust functions the store registers on
-- its connections (crates/store/src/tool_calls.rs).
--
-- workspace_merge: a workspace's work landing on its base, by PR or directly.

CREATE TABLE tool_call (
	id          TEXT PRIMARY KEY,
	tool_use_id TEXT NOT NULL UNIQUE,
	session_id  TEXT NOT NULL,
	message_id  TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
	name        TEXT NOT NULL,
	label       TEXT NOT NULL,
	started_at  INTEGER NOT NULL,
	ended_at    INTEGER,
	is_error    INTEGER NOT NULL DEFAULT 0 CHECK (is_error IN (0, 1)),
	no_match    INTEGER NOT NULL DEFAULT 0 CHECK (no_match IN (0, 1)),
	build_lock  INTEGER NOT NULL DEFAULT 0 CHECK (build_lock IN (0, 1))
);

-- tool_call_backfill: the cursor of the background job that fills tool_call
-- from message_content rows up to end_rowid (crates/store/src/tool_calls.rs,
-- backfill_tool_calls). Live writes open and close calls for later rows. The
-- row is deleted once the job passes end_rowid.
CREATE TABLE tool_call_backfill (
	id         INTEGER PRIMARY KEY CHECK (id = 1),
	next_rowid INTEGER NOT NULL,
	end_rowid  INTEGER NOT NULL
);
INSERT INTO tool_call_backfill (id, next_rowid, end_rowid)
SELECT 1, 0, rowid FROM message_content ORDER BY rowid DESC LIMIT 1;

CREATE INDEX idx_tool_call_started ON tool_call (started_at);
CREATE INDEX idx_tool_call_message ON tool_call (message_id);

CREATE TABLE workspace_merge (
	id           TEXT PRIMARY KEY,
	workspace_id TEXT NOT NULL,
	via          TEXT NOT NULL CHECK (via IN ('pr', 'direct')),
	pr_id        TEXT,
	merged_at    INTEGER NOT NULL
);
CREATE INDEX idx_workspace_merge_at ON workspace_merge (merged_at);

-- Events (escalations among them) by type and time; built first so the
-- direct-land scan below seeks rather than reads every event.
CREATE INDEX idx_events_type_ts ON events (type, ts);

-- Merged PRs, stamped with their last update (the merge or just after it).
INSERT INTO workspace_merge (id, workspace_id, via, pr_id, merged_at)
SELECT recur_uuid7(t), ws, 'pr', id, t FROM (
	SELECT id, json_extract(doc_json, '$.workspaceId') AS ws,
		CAST(ROUND((julianday(json_extract(doc_json, '$.updatedAt')) - 2440587.5) * 86400000) AS INTEGER) AS t
	FROM prs WHERE json_extract(doc_json, '$.state') = 'Merged'
)
WHERE COALESCE(ws, '') != '' AND t IS NOT NULL;

-- Direct lands were recorded only as outcome text: "Squash and merge into
-- <code>main</code> on disk: no PR." or "… to <code>origin/main</code>.".
-- Child workspaces folding into their parent are not landings on a base.
INSERT INTO workspace_merge (id, workspace_id, via, pr_id, merged_at)
SELECT recur_uuid7(MIN(ts)), workspace_id, 'direct', NULL, MIN(ts)
FROM (
	SELECT workspace_id, ts, json_extract(payload_json, '$.event.text') AS text
	FROM events
	WHERE type = 'workspace.event' AND json_extract(payload_json, '$.event.kind') = 'outcome'
)
WHERE (text LIKE '% into <code>%</code> on disk%no PR.' OR text LIKE '%merge to <code>%</code>.')
	AND workspace_id NOT IN (SELECT workspace_id FROM workspace_merge)
	AND workspace_id NOT IN (
		SELECT id FROM workspaces WHERE COALESCE(json_extract(doc_json, '$.parentId'), '') != ''
	)
GROUP BY workspace_id;

-- Turn ends (stamped with the provider's duration, or an error), prompts
-- (user rows with no provider; tool_result carriers have one) and
-- compactions, each by time.
CREATE INDEX idx_message_turn_end ON message (created_at)
	WHERE duration_ms IS NOT NULL OR stream_status = 'error';
CREATE INDEX idx_message_prompt ON message (created_at) WHERE role = 'user' AND provider IS NULL;
CREATE INDEX idx_content_compact ON message_content (created_at) WHERE type = 'compact';
