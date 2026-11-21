### Added

- Agents in Agent mode can run long-lived processes (dev servers, watchers, long test runs) in a terminal tab you can see, with any provider. The workspace owns the process, so it keeps running across turns and model switches. When it exits on its own, the chat that started it gets a new turn with the exit code and the last lines of output. Background tabs show an activity icon: closing one hides it and leaves the process running, the activity count in the terminal dock lists hidden ones to reopen or stop, and the **Stop** button in the terminal dock stops it. With **Bypass Permissions** off, Recur asks before an agent starts one. Background processes stop when you quit Recur or archive the workspace; after a restart, the agent is told on its next turn which of its processes the quit stopped. Their output is also saved to a log file in the workspace's data folder.
- Agents can ask to check back later, with any provider: after a delay from 1 minute to 24 hours, the same chat gets a new turn with the agent's reason, shown as a "Check back in 10 min" row. While it waits, the bottom of the chat counts down ("Checking back in 0:42") with the reason. A chat has at most one pending check back, it still fires if Recur was closed when it came due, and closing the chat or archiving the workspace cancels it.
- Agents in Agent mode can create, list and delete scheduled prompts for their project. Schedules an agent creates show up in the Schedules view like your own. With **Bypass Permissions** off, Recur asks before an agent creates or deletes one. Ask and Plan chats can list schedules.

### Changed

- Todos from every model (Claude, Cursor, Codex, and OpenAI-compatible) now land in the workspace's Plan panel as one shared checklist. They survive switching models or chat tabs, and your own edits to the list are kept.
- Claude, Cursor and Codex chats no longer start their own hidden background processes. Agents use Recur's background processes instead, so they show up in the terminal dock and keep running after the turn ends.
- A Plan made only of todos can be edited in the Plan panel without writing a summary first.

### Fixed

- Earlier Claude chats that stopped a background task now show a "Stopped task" row with the command that was stopped, instead of raw JSON.
