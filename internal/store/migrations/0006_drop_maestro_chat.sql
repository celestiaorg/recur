-- Drop the standalone Maestro chat transcript. All LLM chats/sessions now persist
-- to the normalized session store (session / message / message_content) — the
-- single source of truth — and read back via the session transcript. maestro_chat
-- was a separate per-loop chat store duplicating what the session mirror already
-- records (and it only held text, not tool calls or reasoning).
DROP TABLE IF EXISTS maestro_chat;
