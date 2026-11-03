//! Embeds the third-party licenses Settings > Licenses shows: the file `RECUR_LICENSES_MD`
//! names (packaging sets it to `mise run generate:licenses`'s output), else a pointer to it.
fn main() {
    println!("cargo:rerun-if-env-changed=RECUR_LICENSES_MD");
    let text = match std::env::var("RECUR_LICENSES_MD") {
        Ok(path) if !path.is_empty() => {
            // A relative path is from the repo root (build scripts run in the crate's dir).
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(path);
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("RECUR_LICENSES_MD={}: {e}", path.display()))
        }
        _ => "Packaged builds list the third-party licenses here. For a local build, run \
              `mise run generate:licenses` and build with \
              `RECUR_LICENSES_MD=target/licenses.md`."
            .to_string(),
    };
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("licenses.md");
    std::fs::write(out, text).expect("write licenses.md");
}
