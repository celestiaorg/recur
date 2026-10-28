# Vendored crates

## gpui-pre 0.3.9 (isolated views, contained panics)

`gpui-pre/` is gpui-pre 0.3.9 from crates.io (the exact snapshot gpui-kit 0.8 pins, zed@93d7456) with one patch, `gpui-pre.patch`, wired in by `[patch.crates-io]` in `rust/Cargo.toml`. Only `src/`, `build.rs`, `resources/`, the README and license are kept; `Cargo.toml` drops the `[[example]]` / `[[test]]` / `[[bench]]` tables whose files are not vendored. Commit `dddd0fe0` ("rust: vendor gpui-pre 0.3.9 unchanged") is the pristine import, so `git diff dddd0fe0 -- rust/vendor/gpui-pre` is the patch.

### What it adds

`Entity::isolated(style)` / `AnyView::isolated(style)` (Recur calls it through `recur_ui::style::isolated_view`): a cached view that redraws in place. Upstream, `cx.notify()` marks the notified view and every ancestor dirty, and the window renders the root and every uncached view on every frame, so a key in a text field renders the page that hosts it. With the patch:

- **The dirty walk stops at an isolated view** (`Window::mark_view_dirty`). A notify of the view, or of a view inside it, leaves its ancestors clean.
- **A frame whose only dirty views are isolated replays the root** (`draw_roots`): the previous frame's root prepaint and paint are copied as a cached view's are, and each dirty isolated view renders again at its recorded bounds, element id path, text style, content mask, rem size and opacity, between the runs copied before and after it. Hitboxes, key dispatch nodes, deferred draws, tab stops, input handlers and the scene keep their order, so hit-testing, focus and paint order are those of a full frame.
- **The same split applies inside any replayed cached view or deferred draw** (`reuse_prepaint` / `reuse_paint`), so a dirty isolated view under a clean cached ancestor redraws without it on a full frame too.
- **Every cached view's place is tracked per frame** (`Frame::cached_views`), shifted through each replay and written back to its state after the frame, so a later frame that renders an ancestor again replays the right ranges. A record carried through a replay is shifted by the copied run it lies in, chosen by tree order (its start and end each): an isolated view that recorded nothing leaves the runs around it touching, where a value alone would put the record on the wrong side (`map_in_run`; `an_isolated_view_that_paints_nothing_keeps_the_replay_in_order`). A replay is checked before it runs (`Window::replayable`): a cached view's recorded ranges must fit the frame they copy and the dirty isolated views inside must sit in order in every counter, or the view (or the root) renders instead; written-back records that do not fit mark their view stale. The real-data smoke hit two such cases on a chat with session cards (`roasted-shortcake`), a range past the previous frame and islands out of order in a paint counter.

Anything else (a notify reaching a non-isolated view, a notify of a non-view entity, `Window::refresh`, a resize, the inspector picking, accessibility on) draws exactly as upstream.

### Exact measured sizes

`Window::request_measured_layout_exact` is `request_measured_layout` without ceiling the measured size to device pixels. Box edges are still snapped after layout, so a box placed after a fractional size keeps the offset WebKit gives it, in the same pass. `recur_ui::exact::measured` uses it for text and the chat's flows (`taffy.rs` `NodeContext::exact`).

### Replayed views

`Window::replayed_view(id)` says whether the frame being drawn replayed a cached view instead of rendering it, and `Window::view_stack` lists the views being drawn. `recur_ui::selection` keeps the items of a replayed view in a scope that spans cached views (a PR conversation's items, a transcript's session cards), whose texts do not register again while they replay.

### Unsnapped bounds

`Window::layout_bounds_unsnapped` is `layout_bounds` before the edges snap to device pixels (`taffy.rs` keeps the unrounded origins it snaps from). `recur_ui::layout_root` reads a layout root's or block's fractional edges from it, so content laid out as its own root lands where in-place layout puts it, in one frame.

### Backdrop blur

`Window::paint_backdrop_blur(bounds, corner_radii, blur_radius)` records a `BackdropBlur` primitive: a Gaussian blur, of standard deviation `blur_radius`, of what was painted under `bounds`, clipped to the rounded rect and the content mask, as CSS `backdrop-filter: blur()`. `recur_ui::style::backdrop_blur` paints one under a fill (the scrims, the lightbox). It takes its draw order from the bounds tree like any primitive, so it sits above everything it overlaps that was painted before it. `Scene::batches_with_backdrop_blurs` yields `SceneBatch::BackdropBlurs` between the primitive batches, and a blur ends the batches before it at its order, so a renderer draws exactly what lies under the blur first. `Scene::batches` is the same sequence without the blurs, so renderers that do not draw them (gpui-pre-wgpu on Linux, the test platform) skip them and only the fill shows; `PrimitiveBatch` is unchanged, which keeps gpui-pre-wgpu building unvendored. The blur is a `Primitive` like the others, so a replay (`Scene::replay`, the cached and isolated views' paint ranges) copies it in order with no extra bookkeeping. `Window::scene_batch_labels` (test support) lists the last frame's batches; `recur-ui`'s `backdrop_blur_spec` checks the split and that replayed roots and cached views keep the blur.

### Nested deferred priority

A deferred draw nested in another takes at least its parent's priority (`Window::defer_draw` against `deferred_priority_floor`, which `prepaint_deferred_draws` sets to the draw being prepainted and an isolated view records and restores). Upstream sorts every deferred draw by its own priority across the window, so a popover inside a modal (`suggest` at 60 in the New Task modal at 190) painted under the modal; CSS keeps it above, as the modal's `z-index` makes a stacking context. `recur-ui`'s `deferred_floor_spec` checks the order.

### Contained panics

A panic must not unwind into a platform callback: AppKit, GCD and GLib frames cannot be unwound through, so the process aborts. Recur's own guards (`recur_ui::guard`) contain renders and the callbacks Recur wraps; the patch contains the rest inside GPUI (`src/unwind.rs`):

- **Leases, the window and the update count are restored on unwind.** `App::update_entity_erased` puts the entity back in the map, `App::update_window_erased` puts the window back in the app, and `App::update` and `GpuiBorrow`'s drop leave the update without flushing effects mid-panic, then the unwind resumes. A guard anywhere above a nested `entity.update` therefore finds every entity and the window in place.
- **Element listeners are contained** at dispatch: mouse (`on_click`, `on_mouse_event`, …), key, modifiers and action listeners each run under `unwind::contain`; a panic drops that listener's work and dispatch carries on.
- **`canvas` prepaint and paint are contained** by `Window::contain`, which returns the element id, text style, offset, opacity, content mask, rem size, image cache and rendered entity stacks and the scene's layers to their depths before the closure. The canvas paints nothing that frame.
- **Tasks keep their panic.** gpui-pre's executor wraps every spawned future in `unwind::Recorded`, and the vendored `gpui-pre-scheduler` (below) builds every task with async-task's `propagate_panic`: the task completes with the panic, a detached task drops it, and an awaiter re-raises it inside its own task, which is contained the same way.

Every contained panic is logged (`log::error!`, after Recur's panic hook has written it to `recur.log`) and recorded per thread. `gpui::take_contained_panics()` returns them; GPUI tests (`run_test`) fail when a test leaves any, so a contained panic never hides a failing assertion. `recur-ui`'s `contained_panic_spec` panics in a click handler with a nested update, a key listener, a canvas, foreground and background tasks with an awaiter, and a window update, and checks the app keeps running.

### The contract

An isolated view's size comes from its `style`, never from its content, and anything its host derives from it is not redrawn by its notifies. A view that needs a new size, or a host that shows something derived from the view (a button enabled by a non-empty field), notifies the host for that change only.

### Upgrading

gpui-kit pins gpui-pre exactly. On a gpui-kit bump: replace `gpui-pre/` with the new crates.io source (same file selection, same `Cargo.toml` edit), commit that alone, then `git apply --3way rust/vendor/gpui-pre.patch` and fix what moved. The patch touches `taffy.rs` (exact measured sizes, unsnapped bounds), `window.rs` (frame records, the split replay, the root replay, the dirty walk, contained listeners, `Window::contain`, `paint_backdrop_blur`), `view.rs` (`isolated`, `Island`), `key_dispatch.rs` (`reuse_run` / `reuse_hole` / `finish_reuse` in place of `reuse_subtree`), `text_system/line_layout.rs` (index parts), `app.rs` (restores on unwind), `executor.rs` (`Recorded` futures), `elements/canvas.rs`, `scene.rs` (`layer_depth`, `BackdropBlur`, `SceneBatch`), `test.rs` (failing on contained panics) and the new `unwind.rs`. `recur-ui`'s `isolated_spec`, `contained_panic_spec`, `backdrop_blur_spec` and the `frame_cost` specs are its tests. Drop the vendored copy if upstream gains an equivalent.

Regenerate the patch after editing the vendored source:

```sh
git diff dddd0fe0 -- rust/vendor/gpui-pre > rust/vendor/gpui-pre.patch
```

## gpui-pre-scheduler 0.3.9 (task panics)

`gpui-pre-scheduler/` is the crates.io source of gpui-pre's executor crate (Zed's `scheduler`, same snapshot), wired in by `[patch.crates-io]`. Commit `9386fc71` ("rust: vendor gpui-pre-scheduler 0.3.9 unchanged") is the pristine import (`src/`, `Cargo.toml`, the license), and `gpui-pre-scheduler.patch` is the change: every `async_task::Builder` sets `propagate_panic(true)`, so a panicking task completes with its panic instead of unwinding into the platform dispatcher (see "Contained panics" above). Upgrade it together with gpui-pre: replace the source, commit it alone, `git apply --3way rust/vendor/gpui-pre-scheduler.patch`, and regenerate:

```sh
git diff 9386fc71 -- rust/vendor/gpui-pre-scheduler > rust/vendor/gpui-pre-scheduler.patch
```

## gpui-pre-apple 0.3.9 (border coverage, backdrop blur)

`gpui-pre-apple/` is the crates.io source of GPUI's macOS renderer and text system (same snapshot), wired in by `[patch.crates-io]`. Commit `cb8279c1` ("rust: vendor gpui-pre-apple 0.3.9 unchanged") is the pristine import (`src/`, `build.rs`, `vendor/`, `Cargo.toml`, the license), and `gpui-pre-apple.patch` is the change.

**Border coverage.** The quad shader (`src/shaders.metal`, `quad_fragment`) interpolates between a quad's background and its border in premultiplied space. Upstream mixes straight colors, so toward a transparent background (a card frame, an outline) the border's inner anti-aliased edge carried its coverage twice once blended and came out darker than CoreGraphics paints it: every rounded header inside a bordered card showed the card's background through that edge.

**Backdrop blur.** `MetalRenderer` draws gpui-pre's `BackdropBlur` (above). At a blur batch it ends the frame's render pass and, per blur, renders the covered region (bounds and content mask, whole device pixels) into a scratch texture, blurs it there with two separable Gaussian passes (`backdrop_blur_pass_fragment`, paired taps through one linear sample each), composites the result back through the rounded rect (`backdrop_blur_composite_fragment`), and resumes the frame with `loadAction = Load`. From a 4 device pixel sigma up (CSS `blur(2px)` at scale 2) the first pass averages each 2x2 block and the Gaussian runs at half resolution, within 1.5 levels of an exact blur. The two scratch textures grow to the largest region seen and are reused. A scene with a blur sets the layer's `framebufferOnly` to false for its drawable, which the passes read; frames without one keep it true. Measured on Apple silicon at 2880 by 1800 under load: a full-window `blur(2px)` adds about 0.55ms of GPU time, `blur(8px)` about 1ms.

Upgrade it with gpui-pre: replace the source, commit it alone, `git apply --3way rust/vendor/gpui-pre-apple.patch`, and regenerate:

```sh
git diff cb8279c1 -- rust/vendor/gpui-pre-apple > rust/vendor/gpui-pre-apple.patch
```

## shiguredo_libvpx, shiguredo_aom, shiguredo_opus (Intel Mac source build)

`shiguredo_libvpx/` (2026.2.0-canary.3), `shiguredo_aom/` (2026.3.0) and `shiguredo_opus/` (2026.4.0) are the crates.io releases, wired in by `[patch.crates-io]`. Commit `f1e0965c` is the pristine import, so `git diff f1e0965c -- rust/vendor/shiguredo_*` is the patch.

The crates download prebuilt static libraries for Apple silicon (and Linux, Windows, iOS, Android), and their `source-build` feature configures for the host. The patch adds one case to each `build.rs`: target `x86_64-apple-darwin` (an Intel Mac, built natively or from an Apple silicon host) configures for x86_64 (libvpx `--target=x86_64-darwin20-gcc`; libaom and libopus `CMAKE_OSX_ARCHITECTURES` / `CMAKE_SYSTEM_PROCESSOR` x86_64, libaom `AOM_TARGET_CPU` x86_64) and generates bindings with `--target=x86_64-apple-macosx11.0`. `recur-content` turns `source-build` on for that target only, so the Intel slice of a universal app decodes VP8, VP9, AV1 and Opus as Apple silicon does. The build needs `nasm` (libvpx's and libaom's x86 SIMD) and `llvm-tools` (the crates rename the libraries' symbols; `rust-toolchain.toml`). Drop the patch when the crates publish macOS x86_64 libraries or cross-configure themselves.
