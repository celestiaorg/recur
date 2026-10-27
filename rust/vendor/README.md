# Vendored crates

## gpui-pre 0.3.9 (isolated views, contained panics)

`gpui-pre/` is gpui-pre 0.3.9 from crates.io (the exact snapshot gpui-kit 0.8 pins, zed@93d7456) with one patch, `gpui-pre.patch`, wired in by `[patch.crates-io]` in `rust/Cargo.toml`. Only `src/`, `build.rs`, `resources/`, the README and license are kept; `Cargo.toml` drops the `[[example]]` / `[[test]]` / `[[bench]]` tables whose files are not vendored. Commit `dddd0fe0` ("rust: vendor gpui-pre 0.3.9 unchanged") is the pristine import, so `git diff dddd0fe0 -- rust/vendor/gpui-pre` is the patch.

### What it adds

`Entity::isolated(style)` / `AnyView::isolated(style)` (Recur calls it through `recur_ui::style::isolated_view`): a cached view that redraws in place. Upstream, `cx.notify()` marks the notified view and every ancestor dirty, and the window renders the root and every uncached view on every frame, so a key in a text field renders the page that hosts it. With the patch:

- **The dirty walk stops at an isolated view** (`Window::mark_view_dirty`). A notify of the view, or of a view inside it, leaves its ancestors clean.
- **A frame whose only dirty views are isolated replays the root** (`draw_roots`): the previous frame's root prepaint and paint are copied as a cached view's are, and each dirty isolated view renders again at its recorded bounds, element id path, text style, content mask, rem size and opacity, between the runs copied before and after it. Hitboxes, key dispatch nodes, deferred draws, tab stops, input handlers and the scene keep their order, so hit-testing, focus and paint order are those of a full frame.
- **The same split applies inside any replayed cached view or deferred draw** (`reuse_prepaint` / `reuse_paint`), so a dirty isolated view under a clean cached ancestor redraws without it on a full frame too.
- **Every cached view's place is tracked per frame** (`Frame::cached_views`), shifted through each replay and written back to its state after the frame, so a later frame that renders an ancestor again replays the right ranges.

Anything else (a notify reaching a non-isolated view, a notify of a non-view entity, `Window::refresh`, a resize, the inspector picking, accessibility on) draws exactly as upstream.

### Exact measured widths

`Window::request_measured_layout_exact_width` is `request_measured_layout` without ceiling the measured width to device pixels (the height still ceils). Box edges are still snapped after layout, so a box placed after a fractional width keeps the offset WebKit gives it. `recur_ui::exact::measured` uses it for text, which reports its width ceiled to a LayoutUnit as WebKit does (`taffy.rs` `NodeContext::exact_width`).

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

gpui-kit pins gpui-pre exactly. On a gpui-kit bump: replace `gpui-pre/` with the new crates.io source (same file selection, same `Cargo.toml` edit), commit that alone, then `git apply --3way rust/vendor/gpui-pre.patch` and fix what moved. The patch touches `taffy.rs` (exact measured widths), `window.rs` (frame records, the split replay, the root replay, the dirty walk, contained listeners, `Window::contain`), `view.rs` (`isolated`, `Island`), `key_dispatch.rs` (`reuse_run` / `reuse_hole` / `finish_reuse` in place of `reuse_subtree`), `text_system/line_layout.rs` (index parts), `app.rs` (restores on unwind), `executor.rs` (`Recorded` futures), `elements/canvas.rs`, `scene.rs` (`layer_depth`), `test.rs` (failing on contained panics) and the new `unwind.rs`. `recur-ui`'s `isolated_spec`, `contained_panic_spec` and the `frame_cost` specs are its tests. Drop the vendored copy if upstream gains an equivalent.

Regenerate the patch after editing the vendored source:

```sh
git diff dddd0fe0 -- rust/vendor/gpui-pre > rust/vendor/gpui-pre.patch
```

## gpui-pre-scheduler 0.3.9 (task panics)

`gpui-pre-scheduler/` is the crates.io source of gpui-pre's executor crate (Zed's `scheduler`, same snapshot), wired in by `[patch.crates-io]`. Commit `9386fc71` ("rust: vendor gpui-pre-scheduler 0.3.9 unchanged") is the pristine import (`src/`, `Cargo.toml`, the license), and `gpui-pre-scheduler.patch` is the change: every `async_task::Builder` sets `propagate_panic(true)`, so a panicking task completes with its panic instead of unwinding into the platform dispatcher (see "Contained panics" above). Upgrade it together with gpui-pre: replace the source, commit it alone, `git apply --3way rust/vendor/gpui-pre-scheduler.patch`, and regenerate:

```sh
git diff 9386fc71 -- rust/vendor/gpui-pre-scheduler > rust/vendor/gpui-pre-scheduler.patch
```

## gpui-pre-apple 0.3.9 (border coverage)

`gpui-pre-apple/` is the crates.io source of GPUI's macOS renderer and text system (same snapshot), wired in by `[patch.crates-io]`. Commit `cb8279c1` ("rust: vendor gpui-pre-apple 0.3.9 unchanged") is the pristine import (`src/`, `build.rs`, `vendor/`, `Cargo.toml`, the license), and `gpui-pre-apple.patch` is the change: the quad shader (`src/shaders.metal`, `quad_fragment`) interpolates between a quad's background and its border in premultiplied space. Upstream mixes straight colors, so toward a transparent background (a card frame, an outline) the border's inner anti-aliased edge carried its coverage twice once blended and came out darker than CoreGraphics paints it: every rounded header inside a bordered card showed the card's background through that edge. Upgrade it with gpui-pre: replace the source, commit it alone, `git apply --3way rust/vendor/gpui-pre-apple.patch`, and regenerate:

```sh
git diff cb8279c1 -- rust/vendor/gpui-pre-apple > rust/vendor/gpui-pre-apple.patch
```
