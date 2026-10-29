# Follow-ups

Open items carried over from the port's parity checklist, retired when the Go/Svelte app was removed.

- [ ] **Frozen golden fixtures.** The `go_*.json` goldens under `crates/*/tests/golden` and `providers/*/tests/golden` were written by the Go app (generators: the `*.go.txt` files beside them) and can no longer be regenerated. They still pin Rust behavior; when one needs to change, regenerate it from Rust or edit it by hand and drop its generator.
- [ ] **Asset generators that read the Svelte app.** `crates/ui/assets/icons/generate.mjs` reads `src-web/demo` and `crates/textmate/tests/fixtures/differential/generate.cjs` samples Svelte and Go sources. Make the Rust assets the source of truth or retarget them.
