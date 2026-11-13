//! Unpacks `.zip` / `.tar.gz` artifacts in place before the swap, so the
//! helper renames a real binary or `.app` bundle into place.

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::UpdaterError;
use crate::pathutil;

/// Apps ship code, not blobs: these caps leave headroom over a typical
/// `.app` while keeping hostile archives from exhausting disk.
const MAX_ARCHIVE_ENTRIES: usize = 50_000;
const MAX_ARCHIVE_TOTAL_SIZE: u64 = 2 << 30;

#[derive(Debug, PartialEq, Eq)]
enum ArchiveKind {
    None,
    Zip,
    TarGz,
}

/// By extension only: a false positive would silently break the swap.
fn detect_archive(path: &Path) -> ArchiveKind {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.ends_with(".zip") {
        ArchiveKind::Zip
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        ArchiveKind::TarGz
    } else {
        ArchiveKind::None
    }
}

fn fail(msg: impl std::fmt::Display) -> UpdaterError {
    UpdaterError::Failed(format!("updater: {msg}"))
}

/// Creates a fresh directory `<parent>/<prefix><random>`, like `os.MkdirTemp`.
pub(super) fn mkdir_temp(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default();
        let n = nanos ^ (u64::from(std::process::id()) << 32) ^ SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("{prefix}{}", n % 10_000_000_000));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Unpacks an archive at `staged` in place: it expands into a scratch
/// subdirectory, the archive is removed, and its single top-level entry is
/// moved to where the archive was. Returns the new path and whether anything
/// was extracted (non-archives pass through).
///
/// Enforced: exactly one top-level entry; no entry or symlink escaping the
/// extraction root; entry count and total size caps.
pub(super) fn maybe_extract_into(staged: &Path) -> Result<(PathBuf, bool), UpdaterError> {
    let kind = detect_archive(staged);
    if kind == ArchiveKind::None {
        return Ok((staged.to_path_buf(), false));
    }
    let parent = staged.parent().unwrap_or(Path::new("."));
    let scratch =
        mkdir_temp(parent, ".payload-").map_err(|e| fail(format!("extract scratch dir: {e}")))?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&scratch);
    };

    let res = match kind {
        ArchiveKind::Zip => extract_zip(staged, &scratch),
        _ => extract_tar_gz(staged, &scratch),
    };
    if let Err(e) = res {
        cleanup();
        return Err(e);
    }
    let entries: Vec<_> = match std::fs::read_dir(&scratch) {
        Ok(rd) => rd.flatten().collect(),
        Err(e) => {
            cleanup();
            return Err(fail(format!("extract: {e}")));
        }
    };
    if entries.len() != 1 {
        cleanup();
        return Err(fail(format!(
            "archive must contain exactly one top-level entry, got {}",
            entries.len()
        )));
    }
    let name = entries[0].file_name();
    // Only the payload stays in the staging dir; the helper's cleanup walks
    // one level up.
    if let Err(e) = std::fs::remove_file(staged) {
        cleanup();
        return Err(fail(format!("remove staged archive: {e}")));
    }
    let dst = parent.join(&name);
    if let Err(e) = std::fs::rename(scratch.join(&name), &dst) {
        cleanup();
        return Err(fail(format!("promote extracted entry: {e}")));
    }
    cleanup();
    Ok((dst, true))
}

fn file_mode(perm: u32) -> u32 {
    match perm & 0o777 {
        0 => 0o644,
        p => p,
    }
}

fn dir_mode(perm: u32) -> u32 {
    match perm & 0o777 {
        0 => 0o755,
        p => p,
    }
}

fn mkdir_all(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(path)
}

/// Writes one regular entry, enforcing the running total-size budget.
fn write_entry(
    src: &mut dyn Read,
    target: &Path,
    perm: u32,
    written: &mut u64,
    kind: &str,
) -> Result<(), UpdaterError> {
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(file_mode(perm))
        .open(target)
        .map_err(|e| fail(format!("{kind} create: {e}")))?;
    let limit = MAX_ARCHIVE_TOTAL_SIZE - *written + 1;
    let n =
        io::copy(&mut src.take(limit), &mut out).map_err(|e| fail(format!("{kind} copy: {e}")))?;
    drop(out);
    *written += n;
    if *written > MAX_ARCHIVE_TOTAL_SIZE {
        return Err(fail(format!(
            "{kind} uncompressed size exceeds {MAX_ARCHIVE_TOTAL_SIZE} bytes"
        )));
    }
    Ok(())
}

/// Two passes: directories and files first, symlinks last, so no write can
/// be redirected through a planted link (zip-slip via symlink).
fn extract_zip(src: &Path, dst: &Path) -> Result<(), UpdaterError> {
    let f = File::open(src).map_err(|e| fail(format!("zip open: {e}")))?;
    let mut zr = recur_base::zip::Archive::new(f).map_err(|e| fail(format!("zip open: {e}")))?;
    let count = zr.entries().len();
    if count > MAX_ARCHIVE_ENTRIES {
        return Err(fail(format!(
            "zip has {count} entries (cap {MAX_ARCHIVE_ENTRIES})"
        )));
    }
    let root = PathBuf::from(pathutil::clean(&dst.to_string_lossy()));
    let mut symlinks: Vec<(String, PathBuf)> = Vec::new();
    let mut written = 0u64;
    for i in 0..count {
        let e = &zr.entries()[i];
        let (target, mode, is_dir) = (safe_join(&root, &e.name)?, e.mode, e.is_dir());
        let mut f = zr.open(i).map_err(|e| fail(format!("zip read: {e}")))?;
        if mode & 0o170000 == 0o120000 {
            let mut link = String::new();
            f.by_ref()
                .take(4096)
                .read_to_string(&mut link)
                .map_err(|e| fail(format!("zip symlink body: {e}")))?;
            // Reject escapes up front, before any file is written.
            validate_symlink(&link, &target, &root)?;
            symlinks.push((link, target));
        } else if is_dir || mode & 0o170000 == 0o040000 {
            mkdir_all(&target, dir_mode(mode)).map_err(|e| fail(format!("zip mkdir: {e}")))?;
        } else {
            if let Some(p) = target.parent() {
                mkdir_all(p, 0o755).map_err(|e| fail(format!("zip mkdir: {e}")))?;
            }
            write_entry(&mut f, &target, mode, &mut written, "zip")?;
        }
    }
    for (link, target) in symlinks {
        write_symlink(&link, &target, &root)?;
    }
    Ok(())
}

fn extract_tar_gz(src: &Path, dst: &Path) -> Result<(), UpdaterError> {
    let f = File::open(src).map_err(|e| fail(format!("tar.gz open: {e}")))?;
    let mut ar = recur_base::tar::Archive::new(flate2::read::GzDecoder::new(f));
    let root = PathBuf::from(pathutil::clean(&dst.to_string_lossy()));
    let mut symlinks: Vec<(String, PathBuf)> = Vec::new();
    let mut count = 0usize;
    let mut written = 0u64;
    while let Some(mut entry) = ar.next_entry().map_err(|e| fail(format!("tar: {e}")))? {
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(fail(format!(
                "tar has more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let target = safe_join(&root, &entry.path)?;
        let mode = entry.mode;
        match entry.kind {
            recur_base::tar::Kind::Dir => {
                mkdir_all(&target, dir_mode(mode)).map_err(|e| fail(format!("tar mkdir: {e}")))?;
            }
            recur_base::tar::Kind::Symlink => {
                validate_symlink(&entry.link, &target, &root)?;
                symlinks.push((entry.link.clone(), target));
            }
            recur_base::tar::Kind::File => {
                if let Some(p) = target.parent() {
                    mkdir_all(p, 0o755).map_err(|e| fail(format!("tar mkdir: {e}")))?;
                }
                write_entry(&mut entry, &target, mode, &mut written, "tar")?;
            }
            // Devices, FIFOs, hard links: app bundles never contain them.
            recur_base::tar::Kind::Other => {}
        }
    }
    for (link, target) in symlinks {
        write_symlink(&link, &target, &root)?;
    }
    Ok(())
}

/// Checks that creating `target → link` stays inside `root`.
fn validate_symlink(link: &str, target: &Path, root: &Path) -> Result<(), UpdaterError> {
    if link.starts_with('/') {
        return Err(fail(format!("archive symlink has absolute target: {link}")));
    }
    let dir = target.parent().unwrap_or(Path::new("")).to_string_lossy();
    let resolved = pathutil::join(&[&dir, link]);
    match pathutil::rel(&root.to_string_lossy(), &resolved) {
        Some(rel) if !pathutil::escapes(&rel) => Ok(()),
        _ => Err(fail(format!(
            "archive symlink escapes root: {} -> {link}",
            target.display()
        ))),
    }
}

/// Re-validates (defence in depth), then creates the link.
fn write_symlink(link: &str, target: &Path, root: &Path) -> Result<(), UpdaterError> {
    validate_symlink(link, target, root)?;
    if let Some(p) = target.parent() {
        mkdir_all(p, 0o755).map_err(|e| fail(format!("archive symlink mkdir: {e}")))?;
    }
    let _ = std::fs::remove_file(target);
    std::os::unix::fs::symlink(link, target)
        .map_err(|e| fail(format!("archive symlink create: {e}")))
}

/// Resolves `name` under `root`, rejecting entries that escape it.
fn safe_join(root: &Path, name: &str) -> Result<PathBuf, UpdaterError> {
    let clean = pathutil::clean(&name.replace('\\', "/"));
    if clean == "." {
        return Ok(root.to_path_buf());
    }
    let escape = || fail(format!("archive entry escapes root: {name}"));
    if clean.starts_with('/') || clean.starts_with("..") {
        return Err(escape());
    }
    let root_s = root.to_string_lossy();
    let target = pathutil::join(&[&root_s, &clean]);
    match pathutil::rel(&root_s, &target) {
        Some(rel) if !pathutil::escapes(&rel) => Ok(PathBuf::from(target)),
        _ => Err(escape()),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) enum Entry<'a> {
        Dir(&'a str),
        File(&'a str, &'a [u8], u32),
        Symlink(&'a str, &'a str),
    }

    pub(crate) fn zip_bytes(entries: &[Entry]) -> Vec<u8> {
        let mut zw = recur_base::zip::Builder::new(Vec::new());
        for e in entries {
            match e {
                Entry::Dir(name) => zw.dir(name, 0o755).expect("dir"),
                Entry::File(name, body, mode) => zw.file(name, *mode, body).expect("file"),
                Entry::Symlink(name, target) => zw.symlink(name, target).expect("link"),
            }
        }
        zw.finish().expect("finish")
    }

    fn write_zip(path: &Path, entries: &[Entry]) {
        std::fs::write(path, zip_bytes(entries)).expect("write zip");
    }

    fn write_tar_gz(path: &Path, entries: &[Entry]) {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tw = recur_base::tar::Builder::new(gz);
        for e in entries {
            match e {
                Entry::Dir(name) => tw.dir(name, 0o755).expect("dir"),
                Entry::File(name, body, mode) => tw.file(name, *mode, body).expect("file"),
                Entry::Symlink(name, target) => tw.symlink(name, target).expect("link"),
            }
        }
        let gz = tw.finish().expect("tar");
        std::fs::write(path, gz.finish().expect("gz")).expect("write tar.gz");
    }

    /// Writes a tar.gz with a raw (unchecked) entry name, so `../` names can
    /// be built for the slip tests.
    fn write_raw_tar_gz(path: &Path, name: &str, body: &[u8]) {
        write_tar_gz(path, &[Entry::File(name, body, 0o644)]);
    }

    #[test]
    fn zip_app_bundle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("MyApp-darwin.zip");
        write_zip(
            &archive,
            &[
                Entry::Dir("MyApp.app/"),
                Entry::Dir("MyApp.app/Contents/"),
                Entry::File("MyApp.app/Contents/Info.plist", b"<plist/>", 0o644),
                Entry::Dir("MyApp.app/Contents/MacOS/"),
                Entry::File("MyApp.app/Contents/MacOS/MyApp", b"binary", 0o755),
            ],
        );
        let (out, did) = maybe_extract_into(&archive).expect("extract");
        assert!(did);
        assert_eq!(out.file_name().expect("name"), "MyApp.app");
        assert!(!archive.exists(), "archive removed");
        assert_eq!(
            std::fs::read(out.join("Contents/Info.plist")).expect("read"),
            b"<plist/>"
        );
        let mode = std::fs::metadata(out.join("Contents/MacOS/MyApp"))
            .expect("stat")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "binary should be executable, got {mode:o}");
    }

    #[test]
    fn non_archive_passes_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let staged = dir.path().join("binary");
        std::fs::write(&staged, "ELF").expect("write");
        assert_eq!(
            maybe_extract_into(&staged).expect("extract"),
            (staged, false)
        );
    }

    #[test]
    fn tar_gz_flat_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("myapp.tar.gz");
        write_tar_gz(&archive, &[Entry::File("myapp", b"ELF", 0o755)]);
        let (out, did) = maybe_extract_into(&archive).expect("extract");
        assert!(did);
        assert_eq!(out.file_name().expect("name"), "myapp");
        assert_eq!(std::fs::read(&out).expect("read"), b"ELF");
    }

    #[test]
    fn multiple_top_level_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("two.zip");
        write_zip(
            &archive,
            &[
                Entry::File("one", b"a", 0o644),
                Entry::File("two", b"b", 0o644),
            ],
        );
        let err = maybe_extract_into(&archive).expect_err("two entries");
        assert!(err.to_string().contains("one top-level entry"), "{err}");
    }

    #[test]
    fn zip_slip_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("evil.zip");
        write_zip(&archive, &[Entry::File("../escape.txt", b"pwned", 0o644)]);
        let err = maybe_extract_into(&archive).expect_err("slip");
        assert!(err.to_string().contains("escapes root"), "{err}");
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn tar_slip_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("evil.tar.gz");
        write_raw_tar_gz(&archive, "../escape", b"pwned");
        let err = maybe_extract_into(&archive).expect_err("slip");
        assert!(err.to_string().contains("escapes root"), "{err}");
    }

    #[test]
    fn tar_symlink_escape_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("evil.tar.gz");
        write_tar_gz(
            &archive,
            &[
                Entry::Dir("bundle/"),
                Entry::Symlink("bundle/link", "../../../../etc/passwd"),
            ],
        );
        let err = maybe_extract_into(&archive).expect_err("escape");
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn tar_internal_symlink_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("bundle.tar.gz");
        write_tar_gz(
            &archive,
            &[
                Entry::Dir("bundle/"),
                Entry::File("bundle/real", b"payload", 0o644),
                Entry::Symlink("bundle/alias", "real"),
            ],
        );
        let (out, _) = maybe_extract_into(&archive).expect("extract");
        assert_eq!(
            std::fs::read_link(out.join("alias")).expect("readlink"),
            Path::new("real")
        );
    }

    #[test]
    fn zip_symlinks_validated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("links.zip");
        write_zip(
            &archive,
            &[
                Entry::Dir("b/"),
                Entry::File("b/real", b"x", 0o644),
                Entry::Symlink("b/alias", "real"),
            ],
        );
        let (out, _) = maybe_extract_into(&archive).expect("extract");
        assert_eq!(
            std::fs::read_link(out.join("alias")).expect("readlink"),
            Path::new("real")
        );

        let evil = dir.path().join("evil.zip");
        write_zip(&evil, &[Entry::Dir("b/"), Entry::Symlink("b/up", "/etc")]);
        let err = maybe_extract_into(&evil).expect_err("absolute");
        assert!(err.to_string().contains("absolute target"), "{err}");
    }

    #[test]
    fn safe_join_rules() {
        let root = Path::new("/r");
        assert_eq!(safe_join(root, "a/../b").expect("ok"), Path::new("/r/b"));
        assert_eq!(safe_join(root, "").expect("ok"), Path::new("/r"));
        assert_eq!(safe_join(root, r"a\b").expect("ok"), Path::new("/r/a/b"));
        assert!(safe_join(root, "/abs").is_err());
        assert!(safe_join(root, "a/../../x").is_err());
        assert!(
            safe_join(root, "..foo").is_err(),
            "any leading .. is rejected"
        );
    }
}
