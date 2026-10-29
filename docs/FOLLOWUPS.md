# Follow-ups

Open items carried over from the port's parity checklist, retired when the Go/Svelte app was removed.

- [ ] **Pick the regex dialect.** Bundle patterns (`recur_bundles::go`, e.g. `[[expand]] match`) and code search (`recur_code::gore`) translate Go RE2 onto the `regex` crate (ASCII `\d \s \w \b`, `\Q…\E`, literal `[ & ~` in classes) so shipped bundles behaved the same in both apps. Either keep Go RE2 as the documented dialect or switch to `regex` crate syntax, then document it in `docs/bundles.md` and drop the translation if it is no longer needed.
- [ ] **Frozen golden fixtures.** The `go_*.json` goldens under `crates/*/tests/golden` and `providers/*/tests/golden` were written by the Go app (generators: the `*.go.txt` files beside them) and can no longer be regenerated. They still pin Rust behavior; when one needs to change, regenerate it from Rust or edit it by hand and drop its generator.
- [ ] **Asset generators that read the Svelte app.** `crates/ui/assets/icons/generate.mjs` reads `src-web/demo` and `crates/textmate/tests/fixtures/differential/generate.cjs` samples Svelte and Go sources. Make the Rust assets the source of truth or retarget them.
- [ ] **mr-boxington in CI.** Local dev builds go through `mbx` (see `mise.toml`); CI still uses `Swatinem/rust-cache`. Try `jdx/mr-boxington-action@v1` with `mbx clippy` / `mbx test` in `.github/workflows/ci.yml` so pushes to main fill a shared cache and PRs restore from it.
