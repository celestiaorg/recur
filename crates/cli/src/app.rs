//! The `recur` command tree and what each command does.

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use recur_bundles::{
    Report, RunCtx, format_lint_report, format_report, lint_bundles, run_bundle_tests,
};
use recur_domain::path;

use crate::color::{Stream, want_color};
use crate::{CliError, Io};
use crate::{config, create, open, paths, resolve};

/// Stamped at build time from `RECUR_VERSION`; plain cargo builds use the workspace version.
pub const VERSION: &str = match option_env!("RECUR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "recur",
    version = VERSION,
    about = "Open a path in Recur, or run authoring tools",
    long_about = "Recur command-line tools.\n\n  recur .                      open the current folder\n  recur ./path/to/file         open a file or folder\n  recur                        focus Recur (launch if needed)\n  recur bundles test …         author Recur Bundles",
    disable_version_flag = true
)]
pub struct Cli {
    /// user bundles root (env RECUR_BUNDLES_DIR)
    #[arg(long, global = true, value_name = "DIR")]
    bundles_dir: Option<String>,
    /// print version
    #[arg(short = 'v', long, action = clap::ArgAction::Version)]
    version: Option<bool>,
    /// file or folder to open
    #[arg(value_name = "PATH")]
    paths: Vec<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Author and test Recur Bundles",
        long_about = "Create, test, lint, and manage Recur Bundle packs (TOML macros for the editor).",
        subcommand_required = true,
        arg_required_else_help = true
    )]
    Bundles {
        #[command(subcommand)]
        command: Bundles,
    },
    #[command(
        about = "Open a file or folder in Recur",
        long_about = "Open a file or folder in the Recur desktop app.\n\n  recur .\n  recur ./path/to/folder-or-file\n  recur open .\n\nIf Recur is already running, the path is handed to that instance. Otherwise\nthe app is launched (Recur.app) and the path is opened when it is ready."
    )]
    Open {
        /// file or folder to open
        path: Option<String>,
    },
    #[command(about = "Print a shell completion script")]
    Completion { shell: Shell },
}

#[derive(Subcommand)]
enum Bundles {
    #[command(
        about = "Run tests/*.toml for one or more bundles",
        long_about = "Run in-bundle tests through the real engine (TypeScript on JavaScriptCore, shebang via OS).\n\nArgs may be bundle names (resolved under --bundles-dir, ./bundles, shipped) or paths.\nWith no args, tests the current directory (or every child pack).",
        after_help = "Examples:\n  recur bundles test Text\n  recur bundles test ./bundles/Source\n  recur bundles test ~/.recur/bundles/MyPack\n  recur bundles test ./bundles"
    )]
    Test {
        #[arg(value_name = "NAME|PATH")]
        targets: Vec<String>,
    },
    #[command(
        about = "Lint bundle.toml, scripts, and keyEquivalent collisions",
        long_about = "Validate Recur Bundles: manifest schema, TypeScript compile, shebang #!,\nand keyEquivalent parse / reserved-app collisions.\n\nShared chords across commands and snippets are reported as info (allowed; runtime shows a menu).\nArgs resolve like bundles test.",
        after_help = "Examples:\n  recur bundles lint Text\n  recur bundles lint ./bundles\n  recur bundles lint ~/.recur/bundles/MyPack"
    )]
    Lint {
        #[arg(value_name = "NAME|PATH")]
        targets: Vec<String>,
    },
    #[command(
        about = "Scaffold a new bundle under the user bundles directory",
        long_about = "Create a starter Recur Bundle: bundle.toml, a typescript sample, a shebang sample, and a test for each.\n\nDefault location: --bundles-dir / RECUR_BUNDLES_DIR / ~/.recur/bundles/<Name>.",
        after_help = "Examples:\n  recur bundles create MyPack\n  recur bundles create MyPack --dir ./local-bundles"
    )]
    Create {
        #[arg(value_name = "Name")]
        name: String,
        /// parent directory for the new bundle (default: bundles-dir)
        #[arg(long)]
        dir: Option<String>,
        /// replace an existing bundle directory
        #[arg(long)]
        force: bool,
    },
}

/// Runs the parsed command.
pub fn run(cli: Cli, io: &mut Io) -> Result<(), CliError> {
    config::init().map_err(CliError::Msg)?;
    let flag = cli.bundles_dir.as_deref();
    match cli.command {
        None => {
            let Some(first) = cli.paths.first() else {
                return open::run_open("");
            };
            if !looks_like_open_path(first) {
                return Err(CliError::Msg(format!(
                    "unknown command {first:?} for \"recur\""
                )));
            }
            if cli.paths.len() > 1 {
                return Err(CliError::Msg(format!(
                    "accepts at most 1 path argument, received {}",
                    cli.paths.len()
                )));
            }
            open::run_open(first)
        }
        Some(Command::Open { path }) => open::run_open(path.as_deref().unwrap_or_default()),
        Some(Command::Bundles { command }) => match command {
            Bundles::Test { targets: args } => bundles_test(flag, &args, io),
            Bundles::Lint { targets: args } => bundles_lint(flag, &args, io),
            Bundles::Create { name, dir, force } => bundles_create(flag, &name, dir, force, io),
        },
        Some(Command::Completion { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "recur", io.out);
            Ok(())
        }
    }
}

/// An argument that should be opened rather than treated as a mistyped
/// subcommand: `.` / `..`, anything with a separator, or a name on disk.
pub fn looks_like_open_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s == "." || s == ".." {
        return true;
    }
    if s.contains('/') || s.contains('\\') {
        return true;
    }
    std::fs::metadata(s).is_ok() || std::fs::metadata(path::clean(s)).is_ok()
}

fn targets(flag: Option<&str>, args: &[String]) -> Result<Vec<String>, CliError> {
    let user_root = config::bundles_dir(flag).map_err(|e| CliError::Exit(2, Some(e)))?;
    let shipped_root = paths::shipped_bundles_dir().unwrap_or_default();
    let dirs = resolve::resolve_bundle_targets(args, &user_root, &shipped_root)
        .map_err(|e| CliError::Exit(2, Some(e)))?;
    if dirs.is_empty() {
        return Err(CliError::Exit(2, Some("no bundle.toml found".to_string())));
    }
    Ok(dirs)
}

fn temp_cache(prefix: &str) -> Result<tempfile::TempDir, CliError> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .map_err(|e| CliError::Exit(2, Some(e.to_string())))
}

fn bundles_test(flag: Option<&str>, args: &[String], io: &mut Io) -> Result<(), CliError> {
    let dirs = targets(flag, args)?;
    let cache = temp_cache("recur-bundle-test-cache-")?;
    let cache_dir = cache.path().to_string_lossy().into_owned();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Exit(1, Some(e.to_string())))?;
    let ctx = RunCtx::background();
    let mut total = Report::default();
    for dir in &dirs {
        let rep = rt
            .block_on(run_bundle_tests(&ctx, dir, &cache_dir))
            .map_err(|e| CliError::Exit(1, Some(format!("{dir}: {e}"))))?;
        total.results.extend(rep.results);
        total.passed += rep.passed;
        total.failed += rep.failed;
    }
    let _ = io
        .out
        .write_all(format_report(&total, want_color(Stream::Stdout)).as_bytes());
    if !total.ok() {
        return Err(CliError::Exit(1, None));
    }
    Ok(())
}

fn bundles_lint(flag: Option<&str>, args: &[String], io: &mut Io) -> Result<(), CliError> {
    let dirs = targets(flag, args)?;
    let cache = temp_cache("recur-bundle-lint-cache-")?;
    let rep = lint_bundles(&dirs, &cache.path().to_string_lossy());
    let _ = io
        .out
        .write_all(format_lint_report(&rep, want_color(Stream::Stdout)).as_bytes());
    if !rep.ok() {
        return Err(CliError::Exit(1, None));
    }
    Ok(())
}

fn bundles_create(
    flag: Option<&str>,
    name: &str,
    dir: Option<String>,
    force: bool,
    io: &mut Io,
) -> Result<(), CliError> {
    let name = create::normalize_bundle_name(name);
    let dest_parent = match dir.filter(|d| !d.is_empty()) {
        Some(d) => d,
        None => config::bundles_dir(flag).map_err(|e| CliError::Exit(2, Some(e)))?,
    };
    let dest = create::create_bundle(&name, &dest_parent, force)
        .map_err(|e| CliError::Exit(1, Some(e)))?;
    let _ = write!(
        io.out,
        "created {dest}\nsee {dest}/README.md\nnext: recur bundles test {name}\n"
    );
    Ok(())
}
