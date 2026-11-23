### Added

- Watch schedules: give a schedule a shell command, and every line it prints starts a run with those events appended to the prompt. Recur keeps the command running while it is open, restarts it if it exits, and queues events that arrive during a run into one follow-up run. The schedule row shows whether the command is watching, its last error, and its last event.
- Agents can create Watch schedules with `create_schedule`, and edit any of the project's schedules by passing its id. Recur shows you the command and asks before saving it.
