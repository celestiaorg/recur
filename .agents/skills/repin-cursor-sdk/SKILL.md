---
name: repin-cursor-sdk
description: Re-pin Recur's cursor-sdk wheel and refresh attribution minify anchors when PyPI drifts ahead of PINNED_SDK_VERSION. Use when check:cursor-sdk-pin fails, Cursor SDK pin CI fails, or the user asks to bump cursor-sdk / attribution anchors.
---

# Re-pin cursor-sdk

Recur installs a **pinned** `cursor-sdk` wheel (`PINNED_SDK_VERSION` in `providers/cursor/src/install.rs`) and patches minified `@cursor/sdk` bundles so `attributionConfigProvider` honors Recur settings. Bump the pin and anchors together; never fetch "latest" in install.

## When this applies

- `mise run check:cursor-sdk-pin` fails with pin drift
- GitHub Action **Cursor SDK pin** fails
- User asks to bump `cursor-sdk` or fix attribution patch after a Cursor publish

## Checklist

```
Re-pin progress:
- [ ] Note PyPI latest vs PINNED_SDK_VERSION
- [ ] Download new wheel; find createLocalExecutor sites
- [ ] Update sdk_attribution_anchors (cjs + esm)
- [ ] Bump PINNED_SDK_VERSION + the tests' hardcode
- [ ] Add the new anchor pairs to the golden fixture
- [ ] Validate patch against new wheel
- [ ] Confirm pin check is green
```

## Steps

1. **See drift**
   ```bash
   mise run check:cursor-sdk-pin
   ```
   Failure message includes `pinned=` and `pypi_latest=`.

2. **Unpack the new wheel** (replace `VERSION`):
   ```bash
   pip download "cursor-sdk==VERSION" --no-deps -d /tmp/cs
   unzip -q /tmp/cs/*.whl -d /tmp/cs/w
   BRIDGE=/tmp/cs/w/cursor_sdk/_vendor/bridge
   ```

3. **Find minify anchors** in `$BRIDGE/node_modules/@cursor/sdk/dist/{cjs,esm}/*.js`.
   Search for `getAgentSkills:r.workspaceServices.getAgentSkills`.
   Capture the options-object close through the `void 0===…?new …(…):new …(…)` executor construction for **cjs** and **esm** (minifier names change between releases). Up to 1.0.31 it is `…};return void 0===…`; from 1.0.37 it is `…,...i},u=void 0===…;` inside a `const` chain. From 1.3.1 more fields (`powershellUtf8*`) sit between `getAgentSkills` and `...i}`, so capture from `getAgentSkills` through the executor's `;`.

4. **Update code** (all in `providers/cursor`)
   - `sdk_attribution_anchors` in `src/attribution.rs`: both needles and replacements (assign `attributionConfigProvider` on the options object before the executor is constructed: a statement before `return`, or a comma expression when the site is inside a `const` chain; copy the closest existing entry). Also check `sdk_summary_delta_anchors` and `STEER_RUN_MOUNT_OLD`.
   - `PINNED_SDK_VERSION` in `src/install.rs`.
   - The deliberate hardcode in `pinned_sdk_version_in_pypi_url` (`src/install/tests.rs`).
   - The golden fixture `tests/golden/go_cursor_pkg.json`: the `anchors` and `summaryAnchors` arrays must equal the code's `(old, neu)` pairs in order (`golden_tests::anchors_and_constants`). `<`/`>` are escaped as `\u003c`/`\u003e`.
   - Comments that name the old version (anchors, docs).

5. **Validate**
   ```bash
   RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 cargo test -p recur-provider-cursor --lib attribution_patch_against_pinned_wheel -- --nocapture
   mise run check:cursor-sdk-pin
   cargo test -p recur-provider-cursor -p recur-engine
   ```

6. **Ship** a focused PR: pin bump + anchor refresh + golden fixture + test hardcode. Mention the new version in the summary.

## Do not

- Point Install at PyPI "latest"
- Soft-fail the attribution patch on Recur-owned bridges
- Patch non-owned roots (`$RECUR_CURSOR_BRIDGE`, pip installs)

## Related

- `docs/cursor-provider.md`: bridge install + attribution
- `providers/cursor/src/attribution.rs`: patch implementation
- Env: `RECUR_CURSOR_SDK_PIN_CHECK`, `RECUR_CURSOR_ATTRIBUTION_PATCH_TEST`
