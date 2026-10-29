-- Reverse lookup for fork-aware session delete: "does any other session still
-- join this message?" The PK is (session_id, message_id), so a probe by
-- message_id alone could not use it.
CREATE INDEX IF NOT EXISTS idx_session_message_msg ON session_message (message_id);
