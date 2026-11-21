-- 0019_chat_wakeups: an agent's pending "check back later" on its own chat
-- (schedule_wakeup). One per chat: a new call replaces the old one. The
-- ticker claims due rows by deleting them, so a restart fires each once.

CREATE TABLE chat_wakeup (
	workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
	chat         TEXT NOT NULL,
	-- ISO 8601 UTC text with milliseconds, like scheduled_prompt.next_run_at.
	due_at       TEXT NOT NULL,
	reason       TEXT NOT NULL,
	PRIMARY KEY (workspace_id, chat)
);
CREATE INDEX chat_wakeup_due ON chat_wakeup(due_at);
