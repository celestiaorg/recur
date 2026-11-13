//! The Recur CLI: open paths in the desktop app, read and write its settings, and author
//! Recur Bundles.

mod app;
mod color;
mod config;
mod create;
pub mod open;
mod paths;
mod resolve;
mod settings;

use std::io::Write;

use clap::Parser;
use clap::error::ErrorKind;

pub use app::{VERSION, looks_like_open_path};

/// A command failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// A plain error: printed red, exit 1.
    Msg(String),
    /// An optional message and an explicit exit code.
    Exit(i32, Option<String>),
}

/// The streams a run writes to.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Runs `recur <args>` and returns the process exit code.
pub fn execute(args: &[String], io: &mut Io) -> i32 {
    let argv = std::iter::once("recur").chain(args.iter().map(String::as_str));
    let cli = match app::Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => return clap_exit(err, io),
    };
    match app::run(cli, io) {
        Ok(()) => 0,
        Err(err) => exit_code(err, io),
    }
}

/// Help and version go to stdout with exit 0; usage errors to stderr with
/// exit 1.
fn clap_exit(err: clap::Error, io: &mut Io) -> i32 {
    let (code, w, stream) = match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            (0, &mut *io.out, color::Stream::Stdout)
        }
        _ => (1, &mut *io.err, color::Stream::Stderr),
    };
    let text = err.render();
    let _ = if color::want_color(stream) {
        write!(w, "{}", text.ansi())
    } else {
        write!(w, "{text}")
    };
    code
}

fn exit_code(err: CliError, io: &mut Io) -> i32 {
    let color = color::want_color(color::Stream::Stderr);
    let (code, msg) = match err {
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
