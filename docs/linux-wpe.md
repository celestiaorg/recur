# Linux web content on WPE WebKit (tabled)

Status: spike landed, follow-up tabled. Nothing here blocks the Rust port. Pick it up before in-app browsing (dev sites) ships, since that has to work on Linux.

## Why

The Linux HTML preview (`crates/content/src/html_gtk.rs`) is an offscreen WebKitGTK page: snapshots every 100ms (about 10fps) and input replayed by a JavaScript shim (`INPUT_JS`). GTK 4 cannot send a widget trusted input, so `isTrusted` is false, user-gesture APIs fail, there is no IME and iframes are refused. That cannot carry a browser.

Options considered: a native child window (X11 only, and overlays end up under it), Servo (not compatible enough yet), CEF (strong fallback: Chromium compatibility and DevTools, but about 250MB, a 4-week security cadence and an AppArmor profile for its sandbox on Ubuntu 24.04), and WPE WebKit 2.54 with a custom WPEPlatform. WPE was picked: it is the same engine as the macOS WKWebView, so pages and the Web Inspector behave alike on both.

## What exists

- `build/linux/wpe.Dockerfile`: WPE 2.54.2 built from source (WPEPlatform only, headless) into `/opt/recur/wpe`. Ubuntu 24.04 ships no WPE WebKit.
- `crates/content/src/wpe/platform.c`: the custom platform (about 400 lines of C). It reports no GPU device, so frames arrive as SHM and go through the existing `RenderImage` path.
- `crates/content/build.rs`: compiles the shim only when `pkg-config` finds WPE 2.54 and sets `cfg(recur_wpe)`. `html_wpe.rs` loads it with `RTLD_DEEPBIND` because WebKitGTK in the same process exports the same symbols.
- `RECUR_HTML_WPE=1` picks the WPE page in `html.rs` (`LinuxPage`). The default stays WebKitGTK.
- Checks: `mise run linux:test:wpe` (`crates/content/tests/wpe.rs`).

## Spike results (Docker, 2026-10-28)

| Check | Result |
|---|---|
| Trusted input | Pass: click and keydown `isTrusted`, `navigator.userActivation.isActive` true |
| Typing and IME | Pass in WebKit (composition events with "日本"); GPUI's input handler not wired |
| Hover and `<select>` | Pass, from WebKit itself |
| Animation | 54 fps delivered at 800x600 (page rAF 54.9), about 55 at 2x; the shim gets about 10 |
| https iframe, navigation, popups | iframe loads; `window.open` and leaving `about:blank` refused |
| Sandbox | Fails under Docker's default seccomp; passes with `seccomp=unconfined` and `systempaths=unconfined` |
| Size, build | 127 MB stripped, 54 MB `.tar.gz`, no new runtime packages; 26.5 min build on 16 arm64 cores |

## Follow-up, in order

1. **Ubuntu 24.04 sandbox.** Run bubblewrap under Ubuntu's AppArmor user namespace restriction on a real host (OrbStack's kernel has no AppArmor). If it fails, decide between shipping an AppArmor profile in the `.deb` and relying on the system `bwrap` profile.
2. **In a running window.** Try it through `mise run linux:run` with `RECUR_HTML_WPE=1`: scrolling, find, overlays, resize, HiDPI, parking.
3. **IME.** Connect GPUI's input handler to the platform's input method class.
4. **Frame timing.** Add a `WPEScreen` with the display's refresh rate (rAF runs near 55 without it). Mouse and key events queue separately in WebKit, so a key right after a click can land first.
5. **GPU frames.** Import DMA-BUF frames instead of copying SHM. That needs gpui-pre-wgpu vendored, which Linux backdrop blur needs too (the wgpu renderer skips `SceneBatch::BackdropBlurs` today).
6. **Packaging.** Build amd64 (only arm64 is measured), cache the WPE image in CI, add `/opt/recur/wpe` to `linux:package` (tarball, `.deb`, `.rpm`), and plan WebKit security updates.
7. **Retire WebKitGTK** for web content once WPE is the default, which also drops `RTLD_DEEPBIND`. WebKitGTK still runs mermaid.js and pdf.js; move those too or keep both.

Known pre-existing failure seen during the spike, unrelated: `preview::tests::a_long_markdown_preview_measures_each_text_a_few_times`.
