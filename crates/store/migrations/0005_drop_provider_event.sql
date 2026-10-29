-- Drop the raw provider-line mirror. The normalized session store
-- (message / message_content) is the single source of truth: it now captures
-- thinking blocks too, and any block type it doesn't model round-trips verbatim
-- via a raw content block — so this verbatim NDJSON copy was write-only dead
-- weight (nothing in the app read it) and pure double-storage.
DROP INDEX IF EXISTS idx_provider_event;
DROP TABLE IF EXISTS provider_event;
