//! Deprecated: use `recur bundles ...`. Forwards for compatibility with the
//! earlier `go run ./cmd/bundles` docs (Go `cmd/bundles`).

use std::io::Write;

fn main() {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let _ = writeln!(
        err,
        "note: use `recur bundles ...` (go run ./cmd/recur bundles ...)"
    );
    let code = recur_cli::execute_bundles_forward(
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
