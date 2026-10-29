-- Expand findings from a JSON blob into columns, including public find_…
-- short_id (unique per workspace). Notes become their own table so a finding
-- is a real row, not a document. Empty short_id is backfilled in Go after
-- migrate so the partial unique index can be created here.

CREATE TABLE findings_new (
	id            TEXT PRIMARY KEY,
	short_id      TEXT NOT NULL DEFAULT '',
	workspace_id  TEXT NOT NULL,
	iteration     INTEGER NOT NULL DEFAULT 0,
	file          TEXT NOT NULL DEFAULT '',
	line          INTEGER NOT NULL DEFAULT 0,
	end_line      INTEGER NOT NULL DEFAULT 0,
	severity      TEXT NOT NULL DEFAULT '',
	message       TEXT NOT NULL DEFAULT '',
	summary       TEXT NOT NULL DEFAULT '',
	status        TEXT NOT NULL DEFAULT 'open',
	status_reason TEXT NOT NULL DEFAULT '',
	created_at    TEXT NOT NULL DEFAULT '',
	updated_by    TEXT NOT NULL DEFAULT '',
	updated_at    TEXT NOT NULL DEFAULT '',
	sort          INTEGER NOT NULL DEFAULT 0
);

INSERT INTO findings_new (
	id, short_id, workspace_id, iteration, file, line, end_line,
	severity, message, summary, status, status_reason,
	created_at, updated_by, updated_at, sort
)
SELECT
	id,
	COALESCE(json_extract(doc_json, '$.shortId'), ''),
	workspace_id,
	CAST(COALESCE(json_extract(doc_json, '$.iteration'), 0) AS INTEGER),
	COALESCE(json_extract(doc_json, '$.file'), ''),
	CAST(COALESCE(json_extract(doc_json, '$.line'), 0) AS INTEGER),
	CAST(COALESCE(json_extract(doc_json, '$.endLine'), 0) AS INTEGER),
	COALESCE(json_extract(doc_json, '$.severity'), ''),
	COALESCE(json_extract(doc_json, '$.message'), ''),
	COALESCE(json_extract(doc_json, '$.summary'), ''),
	COALESCE(NULLIF(json_extract(doc_json, '$.status'), ''), 'open'),
	COALESCE(json_extract(doc_json, '$.statusReason'), ''),
	COALESCE(json_extract(doc_json, '$.createdAt'), ''),
	COALESCE(json_extract(doc_json, '$.updatedBy'), ''),
	COALESCE(json_extract(doc_json, '$.updatedAt'), ''),
	sort
FROM findings;

CREATE TABLE finding_notes (
	id         TEXT PRIMARY KEY,
	finding_id TEXT NOT NULL REFERENCES findings_new(id) ON DELETE CASCADE ON UPDATE CASCADE,
	author     TEXT NOT NULL,
	body       TEXT NOT NULL,
	created_at TEXT NOT NULL,
	sort       INTEGER NOT NULL DEFAULT 0
);

INSERT INTO finding_notes (id, finding_id, author, body, created_at, sort)
SELECT
	COALESCE(json_extract(n.value, '$.id'), ''),
	f.id,
	COALESCE(json_extract(n.value, '$.author'), ''),
	COALESCE(json_extract(n.value, '$.body'), ''),
	COALESCE(json_extract(n.value, '$.createdAt'), ''),
	CAST(n.key AS INTEGER)
FROM findings f, json_each(COALESCE(json_extract(f.doc_json, '$.notes'), '[]')) AS n
WHERE json_extract(n.value, '$.id') IS NOT NULL
  AND json_extract(n.value, '$.id') != '';

DROP TABLE findings;
ALTER TABLE findings_new RENAME TO findings;

CREATE INDEX IF NOT EXISTS idx_findings_workspace ON findings (workspace_id, sort);
CREATE UNIQUE INDEX IF NOT EXISTS idx_findings_workspace_short
	ON findings (workspace_id, short_id)
	WHERE short_id != '';
CREATE INDEX IF NOT EXISTS idx_finding_notes_finding ON finding_notes (finding_id, sort);
