//! Writing a file that this process then executes.
//!
//! A spawn on any thread (fork or `posix_spawn`) copies every open descriptor into the child
//! until it execs, close-on-exec ones included, and exec of a file that some process holds
//! open for writing fails with `ETXTBSY` ("text file busy"). So a script written here and run
//! straight away fails at random whenever another thread spawns a process at the same moment,
//! however the write is closed or synced. The file is written by a short-lived `/bin/sh`
//! instead: no thread of this process ever opens it for writing, so no spawn can inherit it,
//! and the writer has exited before anything runs the file.

use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `contents` to `path` (replacing it) with permission `mode`.
pub fn write_executable(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    mode: u32,
) -> io::Result<()> {
    write(path.as_ref(), contents.as_ref(), mode, false)
}

/// Creates `path`, which must not exist yet (`O_EXCL`), with `contents` and `mode`. An
/// existing file is `ErrorKind::AlreadyExists`, so this fits `tempfile::Builder::make`.
pub fn create_executable(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    mode: u32,
) -> io::Result<()> {
    write(path.as_ref(), contents.as_ref(), mode, true)
}

fn write(path: &Path, contents: &[u8], mode: u32, exclusive: bool) -> io::Result<()> {
    if exclusive && path.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists", path.display()),
        ));
    }
    // `set -C` makes `>` create exclusively, as the check above does not across processes.
    let script = if exclusive {
        r#"umask 077; set -C; cat > "$1" && chmod "$2" "$1""#
    } else {
        r#"umask 077; cat > "$1" && chmod "$2" "$1""#
    };
    let mut child = Command::new("/bin/sh")
        .args(["-c", script, "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let fed = child
        .stdin
        .take()
        .map_or(Ok(()), |mut stdin| stdin.write_all(contents));
    let out = child.wait_with_output()?;
    fed?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let kind = if exclusive && path.exists() {
        io::ErrorKind::AlreadyExists
    } else {
        io::ErrorKind::Other
    };
    Err(io::Error::new(kind, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn writes_runs_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.sh");
        write_executable(&p, b"#!/bin/sh\necho one\n", 0o755).unwrap();
        assert_eq!(p.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        let out = Command::new(&p).output().unwrap();
        assert_eq!(out.stdout, b"one\n");
        write_executable(&p, b"#!/bin/sh\necho two\n", 0o700).unwrap();
        assert_eq!(Command::new(&p).output().unwrap().stdout, b"two\n");
        assert_eq!(p.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn create_refuses_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        create_executable(&p, b"a", 0o700).unwrap();
        let e = create_executable(&p, b"b", 0o700).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), b"a");
    }

    /// The race this crate exists for: many threads writing and running scripts while others
    /// spawn. With `std::fs::write` this fails with "text file busy" on Linux within a few
    /// hundred rounds; here 160 run under the same contention.
    #[test]
    fn scripts_run_while_other_threads_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let d = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        let p = d.join(format!("s{t}-{i}"));
                        write_executable(&p, b"#!/bin/sh\nexit 0\n", 0o755).unwrap();
                        let status = Command::new(&p).status().unwrap();
                        assert!(status.success());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
    }
}
