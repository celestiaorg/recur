# Vendored crates

## gpui-pre 0.3.9 (isolated views, contained panics)

`gpui-pre/` is gpui-pre 0.3.9 from crates.io (the exact snapshot gpui-kit 0.8 pins, zed@93d7456) with one patch, `gpui-pre.patch`, wired in by `[patch.crates-io]` in `Cargo.toml`. Only `src/`, `build.rs`, `resources/`, the README and license are kept; `Cargo.toml` drops the `[[example]]` / `[[test]]` / `[[bench]]` tables whose files are not vendored. Commit `dddd0fe0` ("rust: vendor gpui-pre 0.3.9 unchanged") is the pristine import, so `git diff dddd0fe0 -- vendor/gpui-pre` is the patch.

### What it adds

`Entity::isolated(style)` / `AnyView::isolated(style)` (Recur calls it through `recur_ui::style::isolated_view`): a cached view that redraws in place. Upstream, `cx.notify()` marks the notified view and every ancestor dirty, and the window renders the root and every uncached view on every frame, so a key in a text field renders the page that hosts it. With the patch:

- **The dirty walk stops at an isolated view** (`Window::mark_view_dirty`). A notify of the view, or of a view inside it, leaves its ancestors clean.
- **A frame whose only dirty views are isolated replays the root** (`draw_roots`): the previous frame's root prepaint and paint are copied as a cached view's are, and each dirty isolated view renders again at its recorded bounds, element id path, text style, content mask, rem size and opacity, between the runs copied before and after it. Hitboxes, key dispatch nodes, deferred draws, tab stops, input handlers and the scene keep their order, so hit-testing, focus and paint order are those of a full frame.
- **The same split applies inside any replayed cached view or deferred draw** (`reuse_prepaint` / `reuse_paint`), so a dirty isolated view under a clean cached ancestor redraws without it on a full frame too.
- **Every cached view's place is tracked per frame** (`Frame::cached_views`), shifted through each replay and written back to its state after the frame, so a later frame that renders an ancestor again replays the right ranges. A record carried through a replay is shifted by the copied run it lies in, chosen by tree order (its start and end each): an isolated view that recorded nothing leaves the runs around it touching, where a value alone would put the record on the wrong side (`map_in_run`; `an_isolated_view_that_paints_nothing_keeps_the_replay_in_order`). A replay is checked before it runs (`Window::replayable`): a cached view's recorded ranges must fit the frame they copy and the dirty isolated views inside must sit in order in every counter, or the view (or the root) renders instead; written-back records that do not fit mark their view stale. The real-data smoke hit two such cases on a chat with session cards (`roasted-shortcake`), a range past the previous frame and islands out of order in a paint counter.

Anything else (a notify reaching a non-isolated view, a notify of a non-view entity, `Window::refresh`, a resize, the inspector picking, accessibility on) draws exactly as upstream.

### Replayed views

`Window::replayed_view(id)` says whether the frame being drawn replayed a cached view instead of rendering it, and `Window::view_stack` lists the views being drawn. `recur_ui::selection` keeps the items of a replayed view in a scope that spans cached views (a PR conversation's items, a transcript's session cards), whose texts do not register again while they replay.

`Window::after_next_draw` runs a callback at the end of the draw in progress (or the next one), while `replayed_view` still answers for it, and never on a frame that draws nothing (unlike `on_next_frame`). The macOS HTML preview hides its page once a draw neither paints it nor replays a view around it.

### Native views behind the drawing

`Window::paint_hole(bounds)` records a `Hole` primitive: fully transparent pixels over `bounds` (within the content mask) that replace what was painted under them, so a native view behind GPUI's view shows through. It takes its draw order from the bounds tree like any primitive, so ancestors' backgrounds are cleared and what is painted later (deferred draws, tooltips, their shadows) composites over it with its own alpha. `Scene::batches_with_backdrop_blurs` yields `SceneBatch::Holes` between the primitive batches and `Scene::batches` skips them, as with blurs. A replay copies a hole like any primitive.

`HitboxBehavior::Native` is a hitbox that behaves as `Normal` in GPUI; `Window::native_hit_test(position)` says whether the last frame's topmost hitbox at a point is one, and `PlatformWindow::on_hit_test_native` (a default no-op) hands that answer to the platform, so mouse events there reach the native view and anything GPUI draws over it with a hitbox keeps them. The macOS HTML preview paints a hole and a native hitbox over its WKWebView, which sits behind GPUI's view. A renderer cannot sample the view behind a hole, so `Window::backdrop_blur_over_hole(bounds)` (from `after_next_draw`) gives the largest backdrop blur drawn over one and the preview blurs its WKWebView by it (a Core Image content filter). `recur-ui`'s `a_hole_and_native_hitbox_sit_under_overlays` checks the batch order and the hit test, also through a cached view's replay.

### Backdrop blur

`Window::paint_backdrop_blur(bounds, corner_radii, blur_radius)` records a `BackdropBlur` primitive: a Gaussian blur, of standard deviation `blur_radius`, of what was painted under `bounds`, clipped to the rounded rect and the content mask, as CSS `backdrop-filter: blur()`. `recur_ui::style::backdrop_blur` paints one under a fill (the scrims, the lightbox). It takes its draw order from the bounds tree like any primitive, so it sits above everything it overlaps that was painted before it. `Scene::batches_with_backdrop_blurs` yields `SceneBatch::BackdropBlurs` between the primitive batches, and a blur ends the batches before it at its order, so a renderer draws exactly what lies under the blur first. `Scene::batches` is the same sequence without the blurs, so renderers that do not draw them (gpui-pre-wgpu on Linux, the test platform) skip them and only the fill shows; `PrimitiveBatch` is unchanged, which keeps gpui-pre-wgpu building unvendored. The blur is a `Primitive` like the others, so a replay (`Scene::replay`, the cached and isolated views' paint ranges) copies it in order with no extra bookkeeping. `Window::scene_batch_labels` (test support) lists the last frame's batches; `recur-ui`'s `backdrop_blur_spec` checks the split and that replayed roots and cached views keep the blur.

### Nested deferred priority

A deferred draw nested in another takes at least its parent's priority (`Window::defer_draw` against `deferred_priority_floor`, which `prepaint_deferred_draws` sets to the draw being prepainted and an isolated view records and restores). Upstream sorts every deferred draw by its own priority across the window, so a popover inside a modal (`suggest` at 60 in the New Task modal at 190) painted under the modal; CSS keeps it above, as the modal's `z-index` makes a stacking context. `recur-ui`'s `deferred_floor_spec` checks the order.

### Nested scrolling

A scroller's wheel listener clamps the offset to the last prepaint's maximum and stops the event when the offset moved, so the scroller under the pointer scrolls alone and one at its edge passes the wheel to its ancestors, as the browser chains scrolling. Upstream adds the unclamped delta and lets the event through, so every scroller under the pointer moves at once (a tool's output and the chat transcript around it). The listener registers before the element's mouse listeners, so its own `on_scroll_wheel` still runs. `recur-ui`'s `nested_scroll_spec` checks both.

### UI zoom

`Window::set_ui_zoom(zoom)` zooms the whole window as CSS `zoom` on the root element does: the window lays out in logical pixels of `zoom` platform points, so `Window::scale_factor` is the display's times the zoom and `Window::viewport_size` the content size over it, and every length, glyph, icon and image rasterizes at the zoomed resolution with no change to the views. Platform input (`unzoom_input`: mouse positions, pixel wheel deltas, pinch, touch, file drops), `native_hit_test`, the IME bounds (`PlatformInputHandler`) and `show_window_menu` convert at the boundary; anything else that places a native view converts with `Window::zoom_to_platform` (the HTML preview's WKWebView, the date picker). `recur-ui`'s `ui_zoom_spec` checks the layout, a click and a wheel at zoom 2.

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

gpui-kit pins gpui-pre exactly. On a gpui-kit bump: replace `gpui-pre/` with the new crates.io source (same file selection, same `Cargo.toml` edit), commit that alone, then `git apply --3way vendor/gpui-pre.patch` and fix what moved. The patch touches `window.rs` (frame records, the split replay, the root replay, the dirty walk, contained listeners, `Window::contain`, `paint_backdrop_blur`, `paint_hole`, `backdrop_blur_over_hole`, `HitboxBehavior::Native`, the UI zoom), `platform.rs` (`on_hit_test_native`, zoomed IME bounds), `view.rs` (`isolated`, `Island`), `key_dispatch.rs` (`reuse_run` / `reuse_hole` / `finish_reuse` in place of `reuse_subtree`), `text_system/line_layout.rs` (index parts), `app.rs` (restores on unwind), `executor.rs` (`Recorded` futures), `elements/canvas.rs`, `scene.rs` (`layer_depth`, `BackdropBlur`, `Hole`, `SceneBatch`), `test.rs` (failing on contained panics) and the new `unwind.rs`. `recur-ui`'s `isolated_spec`, `contained_panic_spec`, `backdrop_blur_spec`, `ui_zoom_spec` and the `frame_cost` specs are its tests. Drop the vendored copy if upstream gains an equivalent.

Regenerate the patch after editing the vendored source (the pristine import predates the move from `rust/`, so the diff maps that path):

```sh
export GIT_INDEX_FILE=$(mktemp) && git read-tree HEAD && git add vendor/gpui-pre && tree=$(git write-tree) && unset GIT_INDEX_FILE
git diff --abbrev=8 dddd0fe0:rust/vendor/gpui-pre $tree:vendor/gpui-pre --src-prefix=a/rust/vendor/gpui-pre/ --dst-prefix=b/rust/vendor/gpui-pre/ > vendor/gpui-pre.patch
```

## gpui-pre-scheduler 0.3.9 (task panics)

`gpui-pre-scheduler/` is the crates.io source of gpui-pre's executor crate (Zed's `scheduler`, same snapshot), wired in by `[patch.crates-io]`. Commit `9386fc71` ("rust: vendor gpui-pre-scheduler 0.3.9 unchanged") is the pristine import (`src/`, `Cargo.toml`, the license), and `gpui-pre-scheduler.patch` is the change: every `async_task::Builder` sets `propagate_panic(true)`, so a panicking task completes with its panic instead of unwinding into the platform dispatcher (see "Contained panics" above). Upgrade it together with gpui-pre: replace the source, commit it alone, `git apply --3way vendor/gpui-pre-scheduler.patch`, and regenerate:

```sh
git diff 9386fc71 -- vendor/gpui-pre-scheduler > vendor/gpui-pre-scheduler.patch
```

## gpui-pre-apple 0.3.9 (border coverage, backdrop blur, holes, glyph weight)

`gpui-pre-apple/` is the crates.io source of GPUI's macOS renderer and text system (same snapshot), wired in by `[patch.crates-io]`. Commit `cb8279c1` ("rust: vendor gpui-pre-apple 0.3.9 unchanged") is the pristine import (`src/`, `build.rs`, `vendor/`, `Cargo.toml`, the license), and `gpui-pre-apple.patch` is the change.

**Border coverage.** The quad shader (`src/shaders.metal`, `quad_fragment`) interpolates between a quad's background and its border in premultiplied space. Upstream mixes straight colors, so toward a transparent background (a card frame, an outline) the border's inner anti-aliased edge carried its coverage twice once blended and came out darker than CoreGraphics paints it: every rounded header inside a bordered card showed the card's background through that edge.

**Backdrop blur.** `MetalRenderer` draws gpui-pre's `BackdropBlur` (above). At a blur batch it ends the frame's render pass and, per blur, renders the covered region (bounds and content mask, whole device pixels) into a scratch texture, blurs it there with two separable Gaussian passes (`backdrop_blur_pass_fragment`, paired taps through one linear sample each), composites the result back through the rounded rect (`backdrop_blur_composite_fragment`), and resumes the frame with `loadAction = Load`. From a 4 device pixel sigma up (CSS `blur(2px)` at scale 2) the first pass averages each 2x2 block and the Gaussian runs at half resolution, within 1.5 levels of an exact blur. The two scratch textures grow to the largest region seen and are reused. A scene with a blur sets the layer's `framebufferOnly` to false for its drawable, which the passes read; frames without one keep it true. Measured on Apple silicon at 2880 by 1800 under load: a full-window `blur(2px)` adds about 0.55ms of GPU time, `blur(8px)` about 1ms. The composite blends premultiplied (the blurred pixels, alpha included, scaled by coverage), so a blur over a hole stays transparent there instead of painting it opaque black; over opaque pixels it is the same blend as before.

**Holes.** `MetalRenderer` draws gpui-pre's `Hole` (above) inside the frame's render pass, an unblended quad over the clipped bounds that writes transparent pixels (`hole_vertex` / `hole_fragment`). A scene with a hole makes the layer non-opaque for its drawable, so the window composites what is behind it; frames without one keep an opaque window's layer opaque. `a_hole_is_transparent_under_later_paint` renders a background, a hole, a blur and a translucent overlay offscreen and reads the pixels back.

**Glyph weight.** `glyph_dilation_for_color` copies CoreGraphics' font smoothing, which thickens strokes by the text color's luminance, so dark text on a light theme drew thinner than light text on a dark one. It returns the light-text level for every color (still 0 when the user turns smoothing off), so text weighs the same on every theme.

Upgrade it with gpui-pre: replace the source, commit it alone, `git apply --3way vendor/gpui-pre-apple.patch`, and regenerate:

```sh
git diff cb8279c1 -- vendor/gpui-pre-apple > vendor/gpui-pre-apple.patch
```

## gpui-pre-macos, gpui-pre-linux 0.3.9 (zoom cursors, native hit testing)

`gpui-pre-macos/` and `gpui-pre-linux/` are the crates.io sources of GPUI's macOS and Linux platforms (same snapshot), wired in by `[patch.crates-io]` (`src/`, `Cargo.toml`, the license). They map gpui-pre's `CursorStyle::ZoomIn` and `ZoomOut` (CSS `zoom-in` / `zoom-out`: an image that opens a lightbox, the lightbox's actual size): `NSCursor zoomInCursor` / `zoomOutCursor` on macOS 15+ (the pointing hand and the arrow before it), the cursor theme's `zoom-in` / `zoom-out` or Wayland's cursor shapes on Linux. gpui-pre-macos also allows the `deprecated` lint, which crates.io sources had capped, and overrides AppKit's private `-[NSWindow _cornerRadius]` at 10pt, since macOS 26 rounds a window without a toolbar at 16pt.

gpui-pre-macos implements `on_hit_test_native` (above): GPUI's NSView answers `hitTest:` with nil where the topmost hitbox is native, so AppKit hands the click, scroll or drag to the sibling view behind it, and drops its cursor rect while the mouse is over one (`yield_cursor_over_native`, on GPUI's own mouse moves), since WebKit sets its cursor only outside every cursor rect. The answer comes from the window through `try_lock` / `try_borrow_mut`, so a hit test during a GPUI update keeps the event in GPUI. `recur-content`'s `tests/webkit.rs` checks it with a real WKWebView. Commit `f7e929f2` ("rust: vendor gpui-pre-macos and gpui-pre-linux 0.3.9 unchanged") is the pristine import, and `gpui-pre-macos.patch` / `gpui-pre-linux.patch` are the change. Upgrade them with gpui-pre: replace the sources, commit them alone, `git apply --3way` each patch, and regenerate:

```sh
git diff f7e929f2 -- vendor/gpui-pre-macos > vendor/gpui-pre-macos.patch
git diff f7e929f2 -- vendor/gpui-pre-linux > vendor/gpui-pre-linux.patch
```

## gpui-pdf 0.6.1 (no default image formats)

`gpui-pdf/` is gpui-pdf 0.6.1 from crates.io, wired in by `[patch.crates-io]`. Only `src/`, the README and license are kept, and `Cargo.toml` drops the `[[example]]` tables. The patch is one line: its `image` dependency sets `default-features = false`. gpui-pdf only builds an `RgbaImage`, and `image`'s default formats add an AVIF *encoder* (ravif, rav1e and some 25 more crates, about 75 CPU-seconds of a clean build) that nothing in Recur calls; the formats Recur decodes are enabled by gpui-pre and `recur-content`. Drop the copy when gpui-pdf stops enabling `image`'s defaults (0.7.0 still does).

## shiguredo_libvpx, shiguredo_aom, shiguredo_opus (Intel Mac source build)

`shiguredo_libvpx/` (2026.2.0-canary.3), `shiguredo_aom/` (2026.3.0) and `shiguredo_opus/` (2026.4.0) are the crates.io releases, wired in by `[patch.crates-io]`. Commit `f1e0965c` is the pristine import, so `git diff f1e0965c -- vendor/shiguredo_*` is the patch.

The crates download prebuilt static libraries for Apple silicon (and Linux, Windows, iOS, Android), and their `source-build` feature configures for the host. The patch adds one case to each `build.rs`: target `x86_64-apple-darwin` (an Intel Mac, built natively or from an Apple silicon host) configures for x86_64 (libvpx `--target=x86_64-darwin20-gcc`; libaom and libopus `CMAKE_OSX_ARCHITECTURES` / `CMAKE_SYSTEM_PROCESSOR` x86_64, libaom `AOM_TARGET_CPU` x86_64) and generates bindings with `--target=x86_64-apple-macosx11.0`. `recur-content` turns `source-build` on for that target only, so the Intel slice of a universal app decodes VP8, VP9, AV1 and Opus as Apple silicon does. The build needs `nasm` (libvpx's and libaom's x86 SIMD) and `llvm-tools` (the crates rename the libraries' symbols; `rust-toolchain.toml`). Drop the patch when the crates publish macOS x86_64 libraries or cross-configure themselves.
