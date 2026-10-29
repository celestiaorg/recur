-- The Claude Code catalog lists pinned ids only. Rewrite saved Settings that
-- still hold the old sonnet/haiku aliases so every mode keeps a listed model.
UPDATE settings SET doc_json =
	replace(replace(doc_json,
		'"claude-code/sonnet"', '"claude-code/claude-sonnet-5-5"'),
		'"claude-code/haiku"', '"claude-code/claude-haiku-4-5-20251001"');
