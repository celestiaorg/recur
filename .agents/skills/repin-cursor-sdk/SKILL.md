---
name: repin-cursor-sdk
description: Re-pin Recur's cursor-sdk wheel and refresh attribution minify anchors when PyPI drifts ahead of pinnedSDKVersion. Use when check:cursor-sdk-pin fails, Cursor SDK pin CI fails, or the user asks to bump cursor-sdk / attribution anchors.
---

# Re-pin cursor-sdk

Recur installs a **pinned** `cursor-sdk` wheel (`pinnedSDKVersion` in `internal/cursor/install.go`) and patches minified `@cursor/sdk` bundles so `attributionConfigProvider` honors Recur settings. Bump the pin and anchors together — never fetch "latest" in Install.

## When this applies

- `wails3 task check:cursor-sdk-pin` fails with pin drift
- GitHub Action **Cursor SDK pin** fails
- User asks to bump `cursor-sdk` or fix attribution patch after a Cursor publish

## Checklist

```
Re-pin progress:
- [ ] Note PyPI latest vs pinnedSDKVersion
- [ ] Download new wheel; find createLocalExecutor sites
- [ ] Update sdkAttributionAnchors (cjs + esm)
- [ ] Bump pinnedSDKVersion + install_test hardcode
- [ ] Validate patch against new wheel
- [ ] Confirm pin check is green
```

## Steps

1. **See drift**
   ```bash
   wails3 task check:cursor-sdk-pin
   ```
   Failure message includes `pinned=` and `pypi_latest=`.

2. **Unpack the new wheel** (replace `VERSION`):
   ```bash
   pip download "cursor-sdk==VERSION" --no-deps -d /tmp/cs
   unzip -q /tmp/cs/*.whl -d /tmp/cs/w
   BRIDGE=/tmp/cs/w/cursor_sdk/_vendor/bridge
   ```

3. **Find minify anchors** in `$BRIDGE/node_modules/@cursor/sdk/dist/{cjs,esm}/*.js`.
   Search for `getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===`.
   Capture the full `return void 0===…?new …(…):new …(…)` site for **cjs** and **esm** (minifier names change between releases).

4. **Update code**
   - `sdkAttributionAnchors` in `internal/cursor/attribution.go` — both `old` needles and `neu` replacements (inject `attributionConfigProvider` on the options object before the `return`, same pattern as the existing entries).
   - `pinnedSDKVersion` in `internal/cursor/install.go`.
   - The deliberate hardcode in `TestPinnedSDKVersionInPyPIURL` (`internal/cursor/install_test.go`).
   - Comments that name the old version (anchors, docs).

5. **Validate**
   ```bash
   RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 go test ./internal/cursor -run AgainstPinnedWheel -v
   wails3 task check:cursor-sdk-pin
   go test ./internal/cursor ./internal/engine -count=1
   ```

6. **Ship** a focused PR: pin bump + anchor refresh + test hardcode. Mention the new version in the summary.

## Do not

- Point Install at PyPI "latest"
- Soft-fail the attribution patch on Recur-owned bridges
- Patch non-owned roots (`$RECUR_CURSOR_BRIDGE`, pip installs)

## Related

- `docs/cursor-provider.md` — bridge install + attribution
- `internal/cursor/attribution.go` — patch implementation
- Env: `RECUR_CURSOR_SDK_PIN_CHECK`, `RECUR_CURSOR_ATTRIBUTION_PATCH_TEST`
