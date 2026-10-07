-- Monotonic Local PR number high-water mark. NextPRNumber used to scan live
-- PR rows, so deleting a workspace reclaimed its numbers. This singleton
-- survives PR/workspace/project deletes (ClearAll keeps it too).
--
-- Backfill from CAST(prs.id AS INTEGER): Local PR ids are the issued number
-- as text, and promoteLocalPRToGitHub keeps that id while source/number
-- become GitHub. A source<>'gh' scan of number would miss those rows and
-- the next Local PR would reuse the id, overwriting the promoted row.
-- Native GitHub ids are 'gh-N'; CAST of that is 0, so they do not ratchet
-- the Local sequence. Numbers already deleted are gone; from here they
-- only increase.
CREATE TABLE IF NOT EXISTS pr_seq (
	id          INTEGER PRIMARY KEY CHECK (id = 1),
	last_number INTEGER NOT NULL
);

INSERT INTO pr_seq (id, last_number)
SELECT 1, COALESCE(MAX(CAST(id AS INTEGER)), 0)
FROM prs;
