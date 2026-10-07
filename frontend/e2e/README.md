# Frontend E2E (Playwright)

These tests drive the **real Svelte UI** in headless Chromium against a **mocked
Wails bridge**, so they run deterministically with **no Go backend, no `claude`, and
no native window** — ideal for CI and for catching UI regressions.

## Why a mock bridge?

A Wails v3 app's frontend normally talks to Go over the native runtime bridge. In a
plain browser (all Playwright can drive) those bound calls 404 — Wails only offers
"UI previews". So `e2e/helpers/mock-bridge.ts` intercepts `POST /wails/runtime` and
answers each bound-method call from a fixture. The fixture is the **real store seed
dumped from Go**, so the UI is exercised against the actual data contract.

Method dispatch is keyed by Go method *name* (resolved from the generated bindings at
runtime), so a `wails3 generate bindings` run — which changes the numeric method IDs —
does **not** break the mock.

## Run

```bash
cd frontend
pnpm install --frozen-lockfile
pnpm run e2e:install     # one-time: download the Chromium browser
pnpm run e2e             # run the suite (starts the Vite dev server automatically)
pnpm run e2e:ui          # interactive UI mode
pnpm run e2e:report      # open the last HTML report
```

## Regenerate the fixture

After changing the store seed (`internal/store/seed*.go`) or domain types, refresh
the fixture so the tests reflect the new shapes:

```bash
cd frontend
pnpm run e2e:seed        # = go run ./cmd/seeddump > e2e/fixtures/seed.json
```

## Layout

- `helpers/mock-bridge.ts` — bridge interceptor + per-method resolvers.
- `helpers/fixtures.ts` — shared `test` (auto-installs the bridge; asserts no uncaught
  JS errors per test).
- `fixtures/seed.json` — committed seed data (regenerate with `pnpm run e2e:seed`).
- `*.spec.ts` — the tests.

## What's covered vs not

Covered: rendering, navigation, segment filters, the PR review surface (diff, inline
comments, Approve), the new-loop repo picker, Settings (model selectors, rails). The
mocked bridge means these verify the **frontend** against realistic data — they do
**not** exercise the real Go engine. Backend behavior is covered by the Go test suite
(`go test ./internal/...`) and the opt-in real-`claude` smoke (`RECUR_E2E=1`).
