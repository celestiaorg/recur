//! `recur-cli`: the Recur command line (Go `cmd/recur`).
//!
//! ```text
//! recur .
//! recur ./path/to/folder-or-file
//! recur bundles test Text
//! recur bundles lint ./bundles
//! recur bundles create MyPack
//! ```

fn main() {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let code = recur_cli::execute(
        &recur_cli::os_args(),
        &mut recur_cli::Io {
            out: &mut out,
            err: &mut err,
        },
    );
    drop(out);
    drop(err);
    std::process::exit(code);
}
