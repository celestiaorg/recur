# Vendored crates

## gpui-pre 0.3.9 (isolated views)

`gpui-pre/` is gpui-pre 0.3.9 from crates.io (the exact snapshot gpui-kit 0.8 pins, zed@93d7456) with one patch, `gpui-pre.patch`, wired in by `[patch.crates-io]` in `rust/Cargo.toml`. Only `src/`, `build.rs`, `resources/`, the README and license are kept; `Cargo.toml` drops the `[[example]]` / `[[test]]` / `[[bench]]` tables whose files are not vendored. Commit `dddd0fe0` ("rust: vendor gpui-pre 0.3.9 unchanged") is the pristine import, so `git diff dddd0fe0 -- rust/vendor/gpui-pre` is the patch.

### What it adds

`Entity::isolated(style)` / `AnyView::isolated(style)` (Recur calls it through `recur_ui::style::isolated_view`): a cached view that redraws in place. Upstream, `cx.notify()` marks the notified view and every ancestor dirty, and the window renders the root and every uncached view on every frame, so a key in a text field renders the page that hosts it. With the patch:

- **The dirty walk stops at an isolated view** (`Window::mark_view_dirty`). A notify of the view, or of a view inside it, leaves its ancestors clean.
- **A frame whose only dirty views are isolated replays the root** (`draw_roots`): the previous frame's root prepaint and paint are copied as a cached view's are, and each dirty isolated view renders again at its recorded bounds, element id path, text style, content mask, rem size and opacity, between the runs copied before and after it. Hitboxes, key dispatch nodes, deferred draws, tab stops, input handlers and the scene keep their order, so hit-testing, focus and paint order are those of a full frame.
- **The same split applies inside any replayed cached view or deferred draw** (`reuse_prepaint` / `reuse_paint`), so a dirty isolated view under a clean cached ancestor redraws without it on a full frame too.
- **Every cached view's place is tracked per frame** (`Frame::cached_views`), shifted through each replay and written back to its state after the frame, so a later frame that renders an ancestor again replays the right ranges.

Anything else (a notify reaching a non-isolated view, a notify of a non-view entity, `Window::refresh`, a resize, the inspector picking, accessibility on) draws exactly as upstream.

### The contract

An isolated view's size comes from its `style`, never from its content, and anything its host derives from it is not redrawn by its notifies. A view that needs a new size, or a host that shows something derived from the view (a button enabled by a non-empty field), notifies the host for that change only.

### Upgrading

gpui-kit pins gpui-pre exactly. On a gpui-kit bump: replace `gpui-pre/` with the new crates.io source (same file selection, same `Cargo.toml` edit), commit that alone, then `git apply --3way rust/vendor/gpui-pre.patch` and fix what moved. The patch touches `window.rs` (frame records, the split replay, the root replay, the dirty walk), `view.rs` (`isolated`, `Island`), `key_dispatch.rs` (`reuse_run` / `reuse_hole` / `finish_reuse` in place of `reuse_subtree`) and `text_system/line_layout.rs` (index parts). `recur-ui`'s `isolated_spec` and the `frame_cost` specs are its tests. Drop the vendored copy if upstream gains an equivalent.

Regenerate the patch after editing the vendored source:

```sh
git diff dddd0fe0 -- rust/vendor/gpui-pre > rust/vendor/gpui-pre.patch
```
