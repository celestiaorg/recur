# Follow-ups

Open items carried over from the port's parity checklist, retired when the Go/Svelte app was removed.

- [ ] **Asset generators that read the Svelte app.** `crates/ui/assets/icons/generate.mjs` reads `src-web/demo` and `crates/textmate/tests/fixtures/differential/generate.cjs` samples Svelte and Go sources. Make the Rust assets the source of truth or retarget them.
- [ ] **mr-boxington in CI.** Local dev builds go through `mbx` (see `mise.toml`); CI still uses `Swatinem/rust-cache`. Try `jdx/mr-boxington-action@v1` with `mbx clippy` / `mbx test` in `.github/workflows/ci.yml` so pushes to main fill a shared cache and PRs restore from it.
