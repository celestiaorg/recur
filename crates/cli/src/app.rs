//! The `recur` command tree (`internal/cli/root.go`, `bundles.go`,
//! `open.go`) and what each command does.

use recur_bundles::{
    Report, RunCtx, format_lint_report, format_report, lint_bundles, run_bundle_tests,
};
use recur_domain::path;

use crate::Io;
use crate::cobra::{Action, ArgsRule, CliError, Cmd, CmdId, Cobra, FlagKind};
use crate::color::{Stream, want_color};
use crate::{completion, config, create, open, paths, resolve};

/// Stamped at build time from `RECUR_VERSION`.
pub const VERSION: &str = match option_env!("RECUR_VERSION") {
    Some(v) => v,
    None => "dev",
};

/// `newRootCmd`.
pub fn build() -> Cobra {
    let mut root = Cmd::new(
        "recur [path]",
        "Open a path in Recur, or run authoring tools",
    );
    root.long = "Recur command-line tools.\n\n  recur .                      open the current folder\n  recur ./path/to/file         open a file or folder\n  recur                        focus Recur (launch if needed)\n  recur bundles test …         author Recur Bundles".to_string();
    root.version = VERSION.to_string();
    root.args = ArgsRule::Arbitrary;
    root.persistent_pre_run = true;
    root.action = Some(Action::Root);
    let mut c = Cobra::new(root);
    let root = c.root;
    c.flag(
        root,
        true,
        (
            "bundles-dir",
            None,
            FlagKind::Str,
            "user bundles root (env RECUR_BUNDLES_DIR)",
        ),
    );

    let mut bundles = Cmd::new("bundles", "Author and test Recur Bundles");
    bundles.long =
        "Create, test, lint, and manage Recur Bundle packs (TOML macros for the editor)."
            .to_string();
    let bundles = c.add(root, bundles);

    let mut test = Cmd::new(
        "test [name|path]...",
        "Run tests/*.toml for one or more bundles",
    );
    test.long = "Run in-bundle tests through the real engine (TypeScript via esbuild+node, shebang via OS).\n\nArgs may be bundle names (resolved under --bundles-dir, ./bundles, shipped) or paths.\nWith no args, tests the current directory (or every child pack).".to_string();
    test.example = "  recur bundles test Text\n  recur bundles test ./bundles/Source\n  recur bundles test ~/.recur/bundles/MyPack\n  recur bundles test ./bundles".to_string();
    test.args = ArgsRule::Arbitrary;
    test.action = Some(Action::BundlesTest);
    c.add(bundles, test);

    let mut lint = Cmd::new(
        "lint [name|path]...",
        "Lint bundle.toml, scripts, and keyEquivalent collisions",
    );
    lint.long = "Validate Recur Bundles: manifest schema, TypeScript compile, shebang #!,\nand keyEquivalent parse / reserved-app collisions.\n\nShared chords across commands and snippets are reported as info (allowed; runtime shows a menu).\nArgs resolve like bundles test.".to_string();
    lint.example = "  recur bundles lint Text\n  recur bundles lint ./bundles\n  recur bundles lint ~/.recur/bundles/MyPack".to_string();
    lint.args = ArgsRule::Arbitrary;
    lint.action = Some(Action::BundlesLint);
    c.add(bundles, lint);

    let mut create = Cmd::new(
        "create <Name>",
        "Scaffold a new bundle under the user bundles directory",
    );
    create.long = "Create a starter Recur Bundle: bundle.toml, a typescript sample, a shebang sample, and a test for each.\n\nDefault location: --bundles-dir / RECUR_BUNDLES_DIR / ~/.recur/bundles/<Name>.".to_string();
    create.example =
        "  recur bundles create MyPack\n  recur bundles create MyPack --dir ./local-bundles"
            .to_string();
    create.args = ArgsRule::Exact(1);
    create.action = Some(Action::BundlesCreate);
    let create = c.add(bundles, create);
    c.flag(
        create,
        false,
        (
            "dir",
            None,
            FlagKind::Str,
            "parent directory for the new bundle (default: bundles-dir)",
        ),
    );
    c.flag(
        create,
        false,
        (
            "force",
            None,
            FlagKind::Bool,
            "replace an existing bundle directory",
        ),
    );

    let mut open = Cmd::new("open [path]", "Open a file or folder in Recur");
    open.long = "Open a file or folder in the Recur desktop app.\n\n  recur .\n  recur ./path/to/folder-or-file\n  recur open .\n\nIf Recur is already running, the path is handed to that instance. Otherwise\nthe app is launched (Recur.app) and the path is opened when it is ready.".to_string();
    open.args = ArgsRule::Max(1);
    open.action = Some(Action::Open);
    c.add(root, open);
    c
}

/// Runs command `c` (after cobra parsed flags and validated args).
pub fn run(cobra: &mut Cobra, c: CmdId, io: &mut Io) -> Result<(), CliError> {
    if cobra.has_persistent_pre_run(c) {
        config::init().map_err(CliError::Msg)?;
    }
    let args = cobra.args(c).to_vec();
    let Some(action) = cobra.cmds[c].action else {
        return Err(CliError::Help);
    };
    match action {
        Action::Root => {
            let Some(first) = args.first() else {
                return open::run_open("");
            };
            if !looks_like_open_path(first) {
                return Err(CliError::Msg(format!(
                    "unknown command {:?} for {:?}",
                    first,
                    cobra.command_path(c)
                )));
            }
            if args.len() > 1 {
                return Err(CliError::Msg(format!(
                    "accepts at most 1 path argument, received {}",
                    args.len()
                )));
            }
            open::run_open(first)
        }
        Action::Open => open::run_open(args.first().map(String::as_str).unwrap_or_default()),
        Action::BundlesTest => bundles_test(cobra, &args, io),
        Action::BundlesLint => bundles_lint(cobra, &args, io),
        Action::BundlesCreate => bundles_create(cobra, c, &args, io),
        Action::Help => cobra.run_help(c, io),
        Action::Complete => cobra.run_complete(c, io),
        Action::CompletionBash
        | Action::CompletionZsh
        | Action::CompletionFish
        | Action::CompletionPowershell => {
            let no_desc = cobra
                .flag_named(c, "no-descriptions")
                .is_some_and(|f| cobra.flag_bool(f));
            let name = cobra.name(cobra.root).to_string();
            let script = match action {
                Action::CompletionBash => completion::bash(&name, !no_desc),
                Action::CompletionZsh => completion::zsh(&name, !no_desc),
                Action::CompletionFish => completion::fish(&name, !no_desc),
                _ => completion::powershell(&name, !no_desc),
            };
            io.out
                .write_all(script.as_bytes())
                .map_err(|e| CliError::Msg(e.to_string()))
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

/// The `--bundles-dir` value when it was set on the command line.
fn bundles_dir_flag(cobra: &Cobra) -> Option<String> {
    let id = cobra.flag_named(cobra.root, "bundles-dir")?;
    cobra.flags[id]
        .changed
        .then(|| cobra.flags[id].value.clone())
}

fn targets(cobra: &Cobra, args: &[String]) -> Result<Vec<String>, CliError> {
    let user_root = config::bundles_dir(bundles_dir_flag(cobra).as_deref())
        .map_err(|e| CliError::Exit(2, Some(e)))?;
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

fn bundles_test(cobra: &Cobra, args: &[String], io: &mut Io) -> Result<(), CliError> {
    let dirs = targets(cobra, args)?;
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

fn bundles_lint(cobra: &Cobra, args: &[String], io: &mut Io) -> Result<(), CliError> {
    let dirs = targets(cobra, args)?;
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

fn bundles_create(cobra: &Cobra, c: CmdId, args: &[String], io: &mut Io) -> Result<(), CliError> {
    let name = create::normalize_bundle_name(&args[0]);
    let dir_flag = cobra
        .flag_named(c, "dir")
        .map(|f| cobra.flag_value(f).to_string())
        .unwrap_or_default();
    let force = cobra
        .flag_named(c, "force")
        .is_some_and(|f| cobra.flag_bool(f));
    let dest_parent = if dir_flag.is_empty() {
        config::bundles_dir(bundles_dir_flag(cobra).as_deref())
            .map_err(|e| CliError::Exit(2, Some(e)))?
    } else {
        dir_flag
    };
    let dest = create::create_bundle(&name, &dest_parent, force)
        .map_err(|e| CliError::Exit(1, Some(e)))?;
    let _ = write!(
        io.out,
        "created {dest}\nsee {dest}/README.md\nnext: recur bundles test {name}\n"
    );
    Ok(())
}
