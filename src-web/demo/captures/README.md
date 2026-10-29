# Product screen captures

From the repository root:

```sh
wails3 task capture:splash
wails3 task capture:splash -- --screen workspace --theme charcoal
wails3 task capture:splash -- --output /tmp/recur-screens
node scripts/capture-splash.mjs --list
```

The task installs the demo app's dependencies, builds the capture entry,
starts a temporary local server, and captures the requested screens with
Playwright Chromium. If Chromium is missing, run
`cd src-web/demo && pnpm exec playwright install chromium` once.

Outputs are 2x PNGs, `manifest.json`, and an `index.html` gallery in
`src-web/public/screenshots`. Open the gallery to inspect each image at full
resolution and see its settings. Image viewers use CSS dimensions (pixel size
divided by capture scale), so retina captures do not open at twice the app size. A partial run replaces only the selected
captures and keeps the others in the gallery. A failed readiness check leaves
the previous output intact.

## Dataset

`dataset.json` is the dedicated, fictional product dataset. It owns the project,
workspaces (including fan-out children), PRs, code, diffs, transcripts, loop wave,
review findings, inbox notifications, issues, schedules, and settings.
The example includes two projects, a fan-out with two merged children and one
working child, and a three-file review (+41/−7). `baseFiles` and `files` hold the
before/after source; `diffs` supplies the changed-file rows shared by all views.
The session-refresh example is shared across the workspace, changes, loop, and
PR screens. The interactive showcase uses this dataset too.

Edit the dataset directly to change the story. Keep IDs and references consistent:
`workspaceId` and `prId` choose the featured records; PR `workspaceId` links to its
workspace. PR timestamps are ISO strings; chat timestamps are milliseconds.
The diff, commit, and PR file records should agree. Participants use local
initials, except mycodecrafting (Recur's author), whose avatar lives in
`avatars/` and is inlined at build time. Never reference a remote avatar URL:
captures reject network requests.

## Screen presets

`screens.json` controls composition independently of the data. Each preset has:

- A stable ID, title, and intended site placement.
- Viewport width/height and a capture selector (`.win` for the whole app UI).
- Workspace or PR view, selected chat, file, editor/review surface, and PR tab.
- Sidebar, chat, files-pane, and PR-agent-dock visibility.
- Native panel widths and editor preferences.
- A visible selector and expected text that must be ready before capture.

| Preset | Viewport | Composition |
| --- | --- | --- |
| `workspace` | 1440 × 800 | Sidebar, agent chat, editor, files |
| `changes` | 1440 × 800 | Whole window and sidebar; agent and right files pane closed |
| `loop` | 1440 × 800 | Sidebar, wider loop chat, editor; files pane closed |
| `pull-request` | 1440 × 800 | Sidebar and PR conversation; agent dock closed |
| `inbox` | 1440 × 800 | Needs you, working, and ready-for-review tasks, with Ask Recur open |
| `issues` | 1440 × 800 | GitHub and Linear issues, some linked to workspaces |
| `schedules` | 1440 × 800 | Recurring reviews, dependency audits, and release notes |

All tour presets share a 1440 × 800 window and a 280px sidebar. The capture
runner checks these dimensions so switching views cannot change the app scale
or shift the sidebar. Pane visibility and internal pane widths remain per-view.

Every preset runs in Paper and Charcoal by default. App and editor theme IDs
are independent in the `themes` map. To add a screen, copy a preset, give it a
new ID, adjust its settings, and supply a readiness check for the intended content.
The initial file fixture is Go; add the corresponding grammar to the shared
fixture catalog when introducing another code language.

## Fidelity and repeatability

`main.ts` mounts the unmodified `src/App.svelte` and its SCSS. It does
not mount the simplified showcase shell or apply its hidden-control styles.
Pane choices go through the app's stores and editor-session preferences.
These are browser-rendered app screenshots. `window-chrome.css` fills Wails’
reserved titlebar space with macOS traffic lights; the outer OS window shadow
is outside the captured surface. Cropped presets can omit the titlebar.

Each capture starts in a fresh browser context with a fixed clock, locale, time
zone, pixel ratio, and locally pinned Inter/JetBrains Mono fonts. Font licenses
are in `fonts/` (Latin subsets from Google Fonts). The runner waits for content,
fonts and visible images, verifies pane visibility, disables screenshot animations,
and rejects browser errors and external/native-backend requests.

The manifest records the preset, theme pairing, dimensions, image hash, dataset
and preset hashes, app revision/dirty state, browser version, platform, and clock.
Use the same OS and locked Playwright browser version for pixel comparisons.
No agent, shell command, Git operation, or hosted workspace runs during capture.
