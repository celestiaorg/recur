### Fixed

- Claude Code chats older than about 30 days no longer fail with "No conversation found with session ID". Claude Code deletes old transcripts, so Recur now starts a fresh Claude session and passes along the chat history it keeps itself. Retry fixes chats that already show the error.
