### Added

- Workflows can run a command as a phase: `uses = "run"` runs something like `mise run check` in the worktree, and a failure can send the work back to the build phase with the command's output.
- Agents in Agent and Autopilot modes can install a bundle they wrote with the new `install_bundle` tool. Recur lints it first and asks you to approve every install, showing what it adds and anything that runs shell code. A `reload_bundles` tool reloads bundles and lists the installed workflows, roles, and policies.
