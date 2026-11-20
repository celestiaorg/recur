### Fixed

- Merging a workspace into a remote branch such as `origin/main` no longer goes through your project folder. It works when your local `main` is out of date, has commits of its own, or has uncommitted changes, and your project folder stays on whatever branch it was on. Local `main` fast-forwards afterwards when it safely can.

### Changed

- Recur now needs Git 2.38 or newer. Older Git shows a "Git is too old" alert at launch. Rebase merges onto a remote branch need Git 2.40 or newer.
