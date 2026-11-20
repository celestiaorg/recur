### Added

- **Ask Recur** in Inbox can now direct your work: start a workspace in any project, create a new project (a fresh git repo or a clone), send instructions to any workspace's agent, and relay your answers to agents' questions. It reports back when that work finishes, and can look into any workspace to tell you how a task is going. It still does not edit files or approve permissions.
- Agents can start a standalone workspace in another project by name when you ask them to.
- Mention a workspace or project with `@` in any chat, including Ask Recur, to give the agent its current state.
- Inbox shows the selected workspace's overview beside the list (the first one when you arrive) instead of leaving Inbox: what arrived since you last looked, its questions and permission requests (answer them right there), plan approval, open review findings (**Fix** sends them to the agent), failing checks (**Fix checks** sends them to the agent), what it is doing now (with **Stop**), its latest reply, plan progress and changed files. Finished work offers the next step: merge the PR, mark it ready, open one, or archive the workspace. Ask Recur knows which workspace is open, so "this" and "it" mean that workspace and it can relay instructions to its agent. The overview updates live as the agent works. **Open workspace**, Enter, double-click or ⌘-click opens it. Drag the edge between the list and the overview to resize it; Inbox remembers the width, and a wide list brings back its project, plan, diff and age columns. **Mark unread** (⇧⌘U) flags a workspace to come back to.
- Ask Recur sees PR check results and tells you when checks start failing on a workspace it started or sent instructions to.

### Fixed

- Ask Recur can list your workspaces when there are many of them. With a hundred or so, the list was too long for the agent to read and showed an error instead of the workspace card.
- Ask Recur's extra chat tabs (beyond the first) can now look up your workspaces.
