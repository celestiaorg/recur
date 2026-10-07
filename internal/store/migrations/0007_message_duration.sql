-- Per-turn wall time, so a finished agent turn can show how long it took.
--
-- Sourced from the provider's own result (Claude Code's duration_ms, Cursor's
-- durationMs) rather than derived from our message timestamps, which would
-- measure the mirror's bookkeeping instead of the turn. Stamped on the LAST
-- message of a turn, alongside cost.
ALTER TABLE message ADD COLUMN duration_ms INTEGER;
