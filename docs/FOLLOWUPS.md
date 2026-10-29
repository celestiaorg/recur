# Follow-ups

Open items that are known but not scheduled.

- [ ] **The icon generator reads the website demo.** `crates/ui/assets/icons/generate.mjs` reads `src-web/demo` (`src/lib/icons.ts`, `src/lib/lucide-icons.ts`, which the demo no longer has, and `ProviderIcon.svelte`). Make the Rust assets the source of truth or retarget it.
- [ ] **mr-boxington in CI.** Local dev builds go through `mbx` (see `mise.toml`); CI still uses `Swatinem/rust-cache`. Try `jdx/mr-boxington-action@v1` with `mbx clippy` / `mbx test` in `.github/workflows/ci.yml` so pushes to main fill a shared cache and PRs restore from it.
- [ ] **`fetch` in the TypeScript Worker.** The JavaScriptCore Worker (`crates/bundles/src/js.rs`) has no `fetch`, nor `Headers` / `Request` / `Response` / `Blob` or `crypto.subtle` (documented in `docs/bundles.md`). Adding it means a native that runs the request on `recur-http` and resolves a promise from the worker loop, plus those classes in the prelude. No shipped bundle needs it.
