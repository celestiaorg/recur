---
name: cut-release
description: Cut a Recur release - pick the version, bump it everywhere, date the changelog, commit, tag vX.Y.Z and watch the release workflow publish. Use when the user asks to cut, ship or tag a release, bump the version for a release, or check on a release run.
---

# Cut a release

A `vX.Y.Z` tag push runs `.github/workflows/release.yml`, which builds, signs and publishes everything. This skill is the part before the tag (version, changelog, commit) and the check after it. What the pipeline does and which secrets it reads: `docs/ARCHITECTURE.md` ("Releases") and the header comment of `release.yml`.

## Checklist

```
Release progress:
- [ ] main is current and green
- [ ] Version picked and confirmed with the user
- [ ] mise run version:set X.Y.Z
- [ ] Changelog dated, fresh Unreleased added
- [ ] Release commit on main
- [ ] User OK to push the tag
- [ ] Tag pushed, release run green, release marked latest
```

## 1. Start from a green main

```bash
git switch main && git pull --ff-only
git status --short          # must be clean apart from untracked noise
gh run list --branch main --workflow ci.yml --limit 3
mise run check
```

Do not release over a red `main`. CI cancels superseded runs on `main`, so pushing the release commit while a run is in flight cancels it: wait for it to finish (`gh run watch <run-id> --exit-status`) and prepare the commit locally meanwhile. Last release: `git tag --sort=-v:refname | grep '^v[0-9]' | head -1`.

## 2. Pick the version

Read `## Unreleased` in `src-docs/src/pages/changelog.mdx` and the commits since the last tag (`git log --oneline vLAST..main`). Semver, pre-1.0:

- New features or behavior changes (any `Added`, `Changed`, `Removed`): bump the minor, `0.6.0` -> `0.7.0`.
- Only `Fixed` / `Security`: bump the patch, `0.6.0` -> `0.6.1`.

Propose the number and confirm it with the user before writing anything.

## 3. Bump the version

```bash
mise run version:set X.Y.Z
```

This rewrites the workspace `version` in `Cargo.toml`, the `package.json` files listed in `build/set-version.sh`, and `Cargo.lock` (`cargo update --workspace`). `build/set-version.sh --check` (part of `mise run lint`) fails if any of them drift. Do not edit these by hand.

## 4. Date the changelog

In `src-docs/src/pages/changelog.mdx`, turn `## Unreleased` into the release heading and put an empty `## Unreleased` above it:

```md
## Unreleased

## [X.Y.Z](https://github.com/celestiaorg/recur/releases/tag/vX.Y.Z) - YYYY-MM-DD
```

Use today's date. While there, read the section as a user would:

- Entries say what changed for the user, not what the commit did. Rewrite ones that read like commit subjects.
- Each type appears once, in the order Added, Changed, Removed, Fixed, Security, with no empty headings. Parallel PRs often leave a second `### Fixed` at the bottom of `Unreleased`; merge it up.
- PRs branched before the last release sometimes add their entry to that release's dated section. `git diff vLAST main -- src-docs/src/pages/changelog.mdx` shows any lines added below `## [LAST]`; move them into this release.
- Compare against `git log --oneline vLAST..main` for user-facing changes that never got an entry, and ask the user about any you find rather than inventing wording silently.
- No em dashes (`AGENTS.md`, "Voice").

## 5. Commit on main

```bash
git add Cargo.toml Cargo.lock src-docs/src/pages/changelog.mdx $(git diff --name-only -- '*package.json')
git commit -m "Release X.Y.Z"
```

A new commit, never `--amend`. 0.6.0 went straight onto `main`; if branch protection refuses the push, open a PR instead and, once it merges, tag the merged commit on `main`, not the branch commit.

## 6. Tag and push (ask first)

Pushing the tag publishes to every user through the in-app updater and to the `recur-releases` mirror. Stop and get the user's explicit OK, then:

```bash
git push origin main        # if the release commit is not on origin yet
git tag vX.Y.Z              # on the release commit
git push origin vX.Y.Z
```

The tag must equal the `Cargo.toml` version or `build/version.sh` fails every build job.

## 7. Watch the run

```bash
gh run list --workflow release.yml --limit 1
gh run watch <run-id>
gh release view vX.Y.Z
```

Done when the run is green and the release is published (not a draft) and marked latest, with the DMG and zip, the Linux packages, `SHA256SUMS` and `manifest.json`. Optionally download `manifest.json` and the macOS zip into one directory and run `mise run updater:verify <dir>/manifest.json`.

The release notes are GitHub's generated ones. If the user wants the changelog section there, paste it in with `gh release edit vX.Y.Z --notes-file <file>`.

## If the run fails

- `publish` only runs after every build passes, so a failed build leaves no release behind.
- Flaky job (runner, network, notarization timeout): `gh run rerun <run-id> --failed`. `publish` is safe to rerun: it reuses an existing draft and re-uploads with `--clobber`.
- Needs a code fix, and nothing was published: land the fix on `main`, then, with the user's OK, move the tag (`git push --delete origin vX.Y.Z`, `git tag -f vX.Y.Z <new commit>`, push again).
- Already published: never move the tag. Fix on `main` and cut the next patch release.
- `build-mac-slice (x86_64-apple-darwin)` fails while main's CI was green: CI never builds x86_64 macOS, the only target that builds the codec crates from source (`vendor/README.md`). Reproduce with `rustup target add x86_64-apple-darwin` and `cargo check --locked --target x86_64-apple-darwin -p recur-content`.
- `build-mac` fails in "Import signing certificate and notary credentials" with `HTTP status code: 403. A required agreement is missing or has expired`: Apple has a new or expired agreement. Only the account holder can fix it (sign in at developer.apple.com/account and accept it). Then `gh run rerun <run-id> --failed` within a day, while the mac slice artifacts (`retention-days: 1`) still exist; after that, rerun the whole run.
- Something shipped unsigned or without the updater: a secret is missing. `release.yml`'s header lists them and what each one gates.
