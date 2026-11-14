---
name: ademark-bench
description: Run the ADEMark benchmark (github.com/zqkra/ademark) on a Recur branch in the benchmark macOS VM, set the VM up, recalibrate Recur's bench profile, and read results against other ADEs. Use when the user asks to bench a branch, check Recur's ADEMark numbers, recalibrate after a UI change, or set up the bench VM.
---

# ADEMark bench

ADEMark measures ADEs on a Mac: Track A (speed, memory, CPU; a fake model server answers, no plan usage), Track B (tokens), Track C (orchestration, real Claude). It refuses to measure outside a macOS user named `ademark`, posts real keystrokes and clicks, and moves app data aside, so we run it in a UTM macOS VM whose user is `ademark`.

The harness we run is our fork: `mycodecrafting/ademark`, branch `recur` (upstream plus the Recur adapter, profile and gate, a VM display fallback, and the scroll fix from upstream PR #17). In the VM it lives in `~/ADEMark`.

## Bench a branch

```bash
mise run macos:ademark                                   # package, install, re-pin, Track A smoke, summary
ADEMARK_ADES="super-engineering recur" mise run macos:ademark   # a reference app first
ADEMARK_FULL=1 mise run macos:ademark                    # full Track A, about 30 min per app
```

`ADEMARK_VM` (default `ademark@192.168.64.2`) and `ADEMARK_VM_KEY` (default `~/.ssh/ademark_vm`) select the VM. Use the IPv4 address: the `.local` name resolves to a link-local IPv6 address the agent sandbox cannot reach.

- Keep the host quiet. VM timings follow host load: cargo builds, other workspaces' jobs and a second bench run all skew them. A smoke run is one repetition: compare apps run in the same VM, never the VM against ademark.dev.
- Never start a second run while one is going (the task refuses; check `pgrep -f "bun bench/cli.ts run"` in the VM). Two runs share the replay port and results.
- In a VM `A6 *-energy` reads 0 and the instruction counter is 0; ignore both.

Reference smoke numbers in this VM (2026-11-13, build `fd9637ec`, before the caret-blink fix):

| Test | Recur | super.engineering `1856ebe8` |
|---|---|---|
| A1 cold / A2 relaunch | 1.74 / 0.69 s | 1.76 / 0.66 s |
| A3 new thread | 0.27 s | 0.23 s |
| A4 first word | 0.39 s | 0.94 s |
| A5 memory, 5 idle threads | 154 MB | 119 MB |
| A6 idle CPU front / covered | 2.75 / 0.15% | 0.13 / 0.18% |
| A10 per extra thread | 4.9 MB | 5.5 MB |
| A11 open / switch | 961 / 135 ms | invalid |

Rows say `"valid": false` with an `invalid_reason`. Receipts (frames, `video.mov`, `notes.md`) are under `~/ADEMark/results/<run>/<ade>/<test>/rep-NN/`; `ffmpeg` frames from `video.mov` show what the harness saw.

## Recalibrate after a Recur UI change

The profile is `~/ADEMark/bench/adapters/profiles/recur.json` (all rects in window points of a 1600 x 1000 window). Its `notes` say how each value was found. Drive the timing helper over SSH through a launchd job in the GUI session, so it gets its Accessibility and Screen Recording grants (`~/nat.sh` in the VM):

```bash
~/nat.sh window --pid $(pgrep -x recur) --size 1600x1000 --front
~/nat.sh probe-region --pid $(pgrep -x recur) --search 290,850,560,100 --text ademarkqz   # composerRect
~/nat.sh screenshot --pid $(pgrep -x recur) --out /tmp/s.png
```

Recur lays a thread out from the top, so `threadRect` must cover the whole message column. Commit profile changes in the VM and push them to the fork's `recur` branch.

`~/nat.sh` writes a plist with `ProgramArguments` = `~/ADEMark/bench/native/bin/ademark-native` plus its arguments, `RunAtLoad`, and stdout to a temp file, `launchctl bootstrap gui/$(id -u)`s it, waits, boots it out and prints the output. `~/gui.sh` does the same for a `zsh -c` command (for example `claude auth status`, which reads the login keychain only in the GUI session).

## Re-gate a release for ADEMark

Recur is submitted upstream as zqkra/ademark PR #20, pinned to one version and binary SHA-256 with a passing calibration gate. The site measures only that build, so after a release that should be measured:

1. Install the release's notarized DMG in the VM (`gh release download vX.Y.Z -R celestiaorg/recur -p 'Recur-X.Y.Z-macos-universal.dmg'`, then copy, mount and `ditto` it to `/Applications/Recur.app`).
2. In `~/ADEMark`, re-pin `app.version` and `app.bundleSha256` in `bench/adapters/profiles/recur.json` from `recur.installCheck()` (the `macos:ademark` script shows how), fix the first note's version, and commit. Delete any untracked `bench/adapters/gates/recur.json` left by a failed gate: the gate refuses a dirty tree.
3. From Ghostty in the VM (it holds the grants), with the host quiet: `ADEMARK_AGENT=off bun bench/cli.ts calibrate gate --ade recur`. Five Track A smoke runs, about 2 hours; it stops at the first invalid row and names it.
4. Commit the new `gates/recur.json`, move the branch to the host (`git bundle` over scp; the VM has no GitHub credentials) and push it to the fork's `recur` branch.
5. Update the upstream PR (or open a follow-up) with only Recur's files: `bench/adapters/recur.ts`, `profiles/recur.json`, `gates/recur.json`, and `ADES.md` §3.18 (Version row). Never the VM display fallback or another ADE's files. Keep the title conventional and the body short.

A gate failure is usually a real Recur bug the bench found (focus lost while chats auto-resume, a turn resumed after it had finished): look at the row's `notes.md`, `timeout-window.png` and `video.mov` before touching the profile.

## Set up a VM from scratch

1. UTM, Virtualize, macOS (Apple backend): 10 CPUs, 16 GB, 120 GB disk, display 1920 x 1200 non-Retina (80 ppi; Retina quadruples pixels on the paravirtualized GPU and lags). Then, with UTM quit, set `Virtualization:Balloon` and `Display:0:DynamicResolution` to false in `~/Library/Containers/com.utmapp.UTM/Data/Documents/<vm>.utm/config.plist`; saving in UTM's editor can turn the balloon back on.
2. In Setup Assistant name the account `ademark` (admin). Turn on Remote Login, then from the host `ssh-copy-id -i ~/.ssh/ademark_vm.pub ademark@<ip>` (a plain key, not a hardware `_sk` key).
3. Owner, in System Settings: Lock Screen all Never, password after screen saver Never; Users & Groups auto login as `ademark`. A locked screen stops Ghostty from opening windows mid-run.
4. Tools (temporary passwordless sudo makes this scriptable; remove it afterwards): Command Line Tools via `softwareupdate -i`, Homebrew, `brew install oven-sh/bun/bun mitmproxy node displayplacer codex jq`, `brew install --cask ghostty`, `curl -fsSL https://claude.ai/install.sh | bash`, and `~/.local/bin` on PATH in `~/.zprofile` and `~/.zshrc` (GUI apps find `claude` through a login shell).
5. `git clone -b recur https://github.com/mycodecrafting/ademark ~/ADEMark`, then `bash bench/native/build.sh`, `bun bench/cli.ts account setup --apply`, `sudo zsh bench/ops/install-machine.zsh`, `bun bench/cli.ts machine on --apply`, `bun bench/cli.ts snapshot save --dir snapshots/clean --apply`, `defaults write com.apple.dock autohide -bool true` (else the window stops at 966 pt).
6. Owner: Privacy & Security, give Accessibility and Screen & System Audio Recording to Ghostty and `~/ADEMark/bench/native/bin/ademark-native`. Rebuilding the helper drops its grants: remove and re-add it. In Ghostty: `claude`, `/login`. For Tracks B and C: run `mitmdump --mode local`, allow its network extension, trust `~/.mitmproxy/mitmproxy-ca-cert.pem`.
7. Recur needs no seed: the profile sets model, effort and bypass with `recur-cli config set` after launch (Recur 0.7.0+).

## Other ADEs

- **super.engineering**: onboarding Claude Code, Bypass, Chat UI, Compact; Settings: Enhance prompt, update checks and telemetry off; no project. Seed `.super.engineering/settings.json` and `tool-status.json`. Build `1856ebe8` renamed its CLI from `sc` to `se` (fixed in the fork). Install it as `/Applications/super.engineering.app`.
- **Zeron**: 0.3.4 breaks the harness's Zeron driver (written for 0.2.114). Never kill its engine: Zeron refreshes Claude's single-use OAuth token itself, and killing it mid-refresh logs `claude` out (`claude auth status` via `~/gui.sh` shows it; log in again in Ghostty).

## Track C (orchestration)

C1's trigger line for Recur is the one documented in `src-docs/src/pages/concepts/modes.mdx` (Child workspaces); keep the profile's `orchestrationTrigger` identical to it. A C1 run spends 1.7 to 4.2 M tokens of the Claude plan: ask first.
