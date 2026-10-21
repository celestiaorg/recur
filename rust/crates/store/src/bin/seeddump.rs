//! Prints the e2e seed fixture (see `recur_store::seeddump`):
//!
//!     cargo run -p recur-store --bin seeddump > seed.json
//!
//! It re-runs itself with HOME pointed at an empty temp dir, so the
//! project.toml overlay cannot read `~/code/*/.recur/project.toml` and the
//! settings seed never sees a real Codex login.

use std::process::ExitCode;

const CHILD: &str = "RECUR_SEEDDUMP_CHILD";

fn main() -> ExitCode {
    if std::env::var_os(CHILD).is_some() {
        print!("{}", recur_store::seeddump::dump(&recur_store::Store::new_in_memory()));
        return ExitCode::SUCCESS;
    }
    let run = || -> std::io::Result<std::process::ExitStatus> {
        let home = tempfile::tempdir()?;
        std::process::Command::new(std::env::current_exe()?)
            .env(CHILD, "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .status()
    };
    match run() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("seeddump: {e}");
            ExitCode::FAILURE
        }
    }
}
