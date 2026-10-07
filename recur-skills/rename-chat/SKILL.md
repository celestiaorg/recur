---
name: rename-chat
description: >-
  Rename the current chat to match its focus. Use only when the user invokes
  /rename-chat. Optional text after the command steers the title.
disable-model-invocation: true
---
# Rename Chat

Slash-only. Text after `/rename-chat` is an optional naming hint, not a verbatim title.

Pick a 3–5 word topic title in sentence case: first letter uppercase, rest lowercase except acronyms and proper nouns. Hint steers wording only. Example: hint `billing retries` → `Billing retries`. Avoid "Chat", "Conversation", or "Rename chat". At most 200 characters.

Call `mcp__recur__rename_session` once with that title. Do not ask for confirmation. If the tool fails, say so plainly.
