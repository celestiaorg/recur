//! The Recur CLI: open paths in the desktop app and author Recur Bundles.
//!
//! Help, usage, errors, reports and exit codes are stable byte for byte;
//! [`cobra`] reproduces the Cobra/pflag behavior that requires.

mod app;
pub mod cobra;
mod color;
mod completion;
mod config;
mod create;
pub mod open;
mod paths;
mod resolve;

use std::io::Write;

pub use app::{VERSION, looks_like_open_path};
pub use cobra::CliError;

/// The streams a run writes to (Cobra's out and err writers).
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Runs `recur <args>` and returns the process exit code.
pub fn execute(args: &[String], io: &mut Io) -> i32 {
    let mut cli = app::build();
    match cli.execute_c(args, io, &mut app::run) {
        Ok(()) => 0,
        Err(err) => exit_code(err, io),
    }
}

/// Runs as `recur bundles <args>` (the deprecated `cmd/bundles` binary).
pub fn execute_bundles_forward(args: &[String], io: &mut Io) -> i32 {
    let mut full = vec!["bundles".to_string()];
    full.extend_from_slice(args);
    execute(&full, io)
}

fn exit_code(err: CliError, io: &mut Io) -> i32 {
    let color = color::want_color(color::Stream::Stderr);
    let (code, msg) = match err {
        CliError::Help => (0, None),
        CliError::Msg(m) => (1, Some(m)),
        CliError::Exit(code, m) => (code, m),
    };
    if let Some(m) = msg {
        let _ = writeln!(io.err, "{}", color::paint_red(&m, color));
    }
    code
}

/// Process args as strings (invalid UTF-8 is replaced).
pub fn os_args() -> Vec<String> {
    std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Exposed for tests: a bundle created from the embedded scaffold.
pub fn create_bundle(name: &str, dest_parent: &str, force: bool) -> Result<String, String> {
    create::create_bundle(name, dest_parent, force)
}

/// Exposed for tests: bundle target resolution with explicit roots.
pub fn resolve_bundle_targets(
    args: &[String],
    user_root: &str,
    shipped_root: &str,
) -> Result<Vec<String>, String> {
    resolve::resolve_bundle_targets(args, user_root, shipped_root)
}
