//! Reading, serving and saving files for the observatory editor and chat
//! attachments.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use recur_domain::FileContent;
use tokio::io::AsyncWrite;

use crate::drop_allow::external_path_allowed;
use crate::error::{Error, Result};
use crate::gopath;
use crate::root::{path_str, rel_slash_str, safe_join_str};

/// Text preview/edit cap.
pub(crate) const MAX_READ_BYTES: i64 = 1 << 30;
/// Image preview cap.
pub(crate) const MAX_IMAGE_BYTES: i64 = 512 << 20;
/// Larger text bodies skip the UI bridge and stream instead.
pub(crate) const MAX_INLINE_BYTES: i64 = 256 << 10;
/// Above this the editor is view-only.
pub(crate) const MAX_EDIT_BYTES: usize = 16 << 20;
pub(crate) const TEXT_PROBE_BYTES: usize = 8 << 10;
/// Chat paste / Finder-drop attachment cap.
pub(crate) const MAX_CHAT_IMAGE_BYTES: u64 = 8 << 20;

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// Lowercase extension → MIME type for binary image previews. SVG is
/// editable source with a separate preview mode, so it is not here.
fn image_mime_ext(ext: &str) -> &'static str {
    match ext {
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".ico" => "image/x-icon",
        ".bmp" => "image/bmp",
        ".avif" => "image/avif",
        ".tif" | ".tiff" => "image/tiff",
        _ => "",
    }
}

/// Played in the observatory with a video element; served with Range
/// support, so no size cap applies.
fn video_mime_ext(ext: &str) -> &'static str {
    match ext {
        ".mp4" | ".m4v" => "video/mp4",
        ".mov" => "video/quicktime",
        ".webm" => "video/webm",
        ".ogv" => "video/ogg",
        _ => "",
    }
}

/// The paste / Finder-drop set. Narrower than the preview set: ico, bmp and
/// avif preview in the observatory but cannot be pasted into chat.
fn chat_image_mime_ext(ext: &str) -> &'static str {
    match ext {
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        _ => "",
    }
}

fn lower_ext(path: &str) -> String {
    gopath::ext(path).to_lowercase()
}

/// The preview MIME type for a path, or `""` if it is not a known image.
pub fn image_mime(path: &str) -> &'static str {
    image_mime_ext(&lower_ext(path))
}

/// [`image_mime`] plus PDF and video, for serving file bytes into the
/// observatory preview (not for raster-only operations).
pub fn preview_mime(path: &str) -> &'static str {
    let mime = image_mime(path);
    if !mime.is_empty() {
        return mime;
    }
    let ext = lower_ext(path);
    if ext == ".pdf" {
        return "application/pdf";
    }
    video_mime_ext(&ext)
}

pub(crate) fn preview_limit(mime: &str) -> i64 {
    if mime.starts_with("video/") {
        i64::MAX
    } else if !mime.is_empty() {
        MAX_IMAGE_BYTES
    } else {
        MAX_READ_BYTES
    }
}

fn not_found(rel: &str) -> Error {
    Error::NotFound(rel.to_string())
}

/// Streams an image at the merge-base of `base_branch` and HEAD (HEAD when
/// `base_branch` is empty) into `dst`, for the observatory's previous/current
/// comparison. Empty or missing at that revision is [`Error::NotFound`].
/// `dst` may be `None` to only stat (HEAD requests).
pub async fn image_at_base<W: AsyncWrite + Unpin>(
    root: &Path,
    base_branch: &str,
    rel: &str,
    dst: Option<&mut W>,
) -> Result<()> {
    let root = path_str(root);
    let abs = safe_join_str(&root, rel)?;
    let rel = rel_slash_str(&root, &abs);
    if image_mime(&rel).is_empty() {
        return Err(Error::msg("not an image"));
    }
    let b = base_branch.trim();
    let against = if b.is_empty() {
        "HEAD".to_string()
    } else {
        recur_git::merge_base(&root, b).await
    };
    let size = recur_git::file_size_at_rev(&root, &against, &rel).await?;
    let size = match size {
        Some(n) if n != 0 => n,
        _ => return Err(not_found(&rel)),
    };
    if size > MAX_IMAGE_BYTES {
        return Err(Error::msg(format!("file too large ({size} bytes)")));
    }
    let Some(dst) = dst else {
        return Ok(());
    };
    if !recur_git::copy_file_at_rev(&root, &against, &rel, dst, MAX_IMAGE_BYTES).await? {
        return Err(not_found(&rel));
    }
    Ok(())
}

/// Validates an absolute path the user dropped or pasted as a chat image:
/// allowlisted, regular file, accepted extension, size cap and magic bytes.
/// Returns the cleaned path and its MIME type.
pub fn prepare_chat_image(abs: &str) -> Result<(PathBuf, &'static str)> {
    let clean = check_external(abs)?;
    let mime = chat_image_mime_ext(&lower_ext(&clean));
    if mime.is_empty() {
        return Err(Error::msg("not a chat image"));
    }
    let st = stat_external(&clean)?;
    if st.len() > MAX_CHAT_IMAGE_BYTES {
        return Err(Error::msg(format!("file too large ({} bytes)", st.len())));
    }
    let mut f = File::open(&clean).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            not_found(&clean)
        } else {
            Error::path("open", &clean, e)
        }
    })?;
    let mut buf = [0u8; 12];
    let n = read_full(&mut f, &mut buf).map_err(|e| Error::path("read", &clean, e))?;
    if !chat_image_magic_ok(&buf[..n], mime) {
        return Err(Error::msg("not a chat image"));
    }
    Ok((PathBuf::from(clean), mime))
}

/// Trimmed, NUL-free, absolute, cleaned and allowlisted.
fn check_external(abs: &str) -> Result<String> {
    let abs = abs.trim();
    if abs.is_empty() {
        return Err(Error::msg("missing path"));
    }
    if abs.contains('\0') {
        return Err(Error::msg("invalid path"));
    }
    let clean = gopath::clean(abs);
    if !gopath::is_abs(&clean) {
        return Err(Error::msg("path must be absolute"));
    }
    if !external_path_allowed(&clean) {
        return Err(Error::msg("path not allowed"));
    }
    Ok(clean)
}

/// A regular file at an external path.
fn stat_external(clean: &str) -> Result<fs::Metadata> {
    let st = fs::metadata(clean).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            not_found(clean)
        } else {
            Error::path("stat", clean, e)
        }
    })?;
    if st.is_dir() {
        return Err(Error::msg(format!("{} is a directory", gopath::base(clean))));
    }
    if !st.is_file() {
        return Err(Error::msg("not a regular file"));
    }
    Ok(st)
}

fn chat_image_magic_ok(b: &[u8], mime: &str) -> bool {
    match mime {
        "image/png" => b.len() >= 8 && b[..8] == PNG_MAGIC,
        "image/jpeg" => b.len() >= 3 && b[0] == 0xff && b[1] == 0xd8 && b[2] == 0xff,
        "image/gif" => b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a"),
        "image/webp" => b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP",
        _ => false,
    }
}

/// Go `io.ReadFull` tolerating EOF: the number of bytes read.
fn read_full(f: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Loads a file under `root` for the editor. Text up to 256 KiB is inlined in
/// `content` (one round-trip for a typical source file); larger text and
/// binary previews leave it empty and stream through the file server.
pub fn read_file(root: &Path, rel: &str, writable: bool) -> Result<FileContent> {
    let (full, mime, st) = stat_file(&path_str(root), rel)?;
    let mut out = FileContent {
        path: rel.to_string(),
        size: st.len() as i64,
        writable,
        mime: mime.to_string(),
        mtime: mtime_millis(&st),
        ..Default::default()
    };
    if !mime.is_empty() {
        return Ok(out);
    }
    reject_binary_file(&full, st.len() as i64)?;
    out.language = language_for(rel);
    if st.len() as i64 > MAX_INLINE_BYTES {
        return Ok(out);
    }
    let raw = fs::read(&full).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            not_found(rel)
        } else {
            Error::path("open", &full, e)
        }
    })?;
    let Ok(content) = String::from_utf8(raw) else {
        return Err(Error::msg("binary or non-UTF-8 file"));
    };
    out.size = content.len() as i64;
    out.content = content;
    Ok(out)
}

fn mtime_millis(st: &fs::Metadata) -> i64 {
    match st.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            Err(e) => -(e.duration().as_millis() as i64),
        },
        Err(_) => 0,
    }
}

/// Resolves a file the file server can stream, with [`read_file`]'s sandbox,
/// size and image/text rules, without loading the body. The caller opens the
/// returned path.
pub fn prepare_serve(root: &Path, rel: &str) -> Result<(PathBuf, &'static str)> {
    let (abs, mime, _) = stat_file(&path_str(root), rel)?;
    Ok((PathBuf::from(abs), mime))
}

/// Validates an absolute path for the editor (Finder-dropped files outside
/// the worktree): allowlisted, regular file, preview size rules, no root
/// sandbox.
pub fn prepare_external(abs: &str) -> Result<(PathBuf, &'static str)> {
    let clean = check_external(abs)?;
    let st = stat_external(&clean)?;
    let mime = preview_mime(&clean);
    if st.len() as i64 > preview_limit(mime) {
        return Err(Error::msg(format!("file too large ({} bytes)", st.len())));
    }
    Ok((PathBuf::from(clean), mime))
}

/// Peeks at `f` and seeks back to the start. Non-image files that look
/// binary are rejected with [`read_file`]'s error.
pub fn rewind_binary_check(f: &mut File) -> Result<()> {
    let mut probe = vec![0u8; TEXT_PROBE_BYTES];
    let n = read_full(f, &mut probe).map_err(|e| Error::msg(e.to_string()))?;
    let truncated = n == TEXT_PROBE_BYTES;
    if is_binary(&probe[..n], truncated) {
        return Err(Error::msg("binary or non-UTF-8 file"));
    }
    f.seek(SeekFrom::Start(0))
        .map_err(|e| Error::msg(e.to_string()))?;
    Ok(())
}

fn stat_file(root: &str, rel: &str) -> Result<(String, &'static str, fs::Metadata)> {
    let abs = safe_join_str(root, rel)?;
    let st = fs::metadata(&abs).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            not_found(rel)
        } else {
            Error::path("stat", &abs, e)
        }
    })?;
    if st.is_dir() {
        return Err(Error::msg(format!("{rel} is a directory")));
    }
    let mime = preview_mime(rel);
    if st.len() as i64 > preview_limit(mime) {
        return Err(Error::msg(format!("file too large ({} bytes)", st.len())));
    }
    Ok((abs, mime, st))
}

fn reject_binary_file(full: &str, size: i64) -> Result<()> {
    let n = size.min(TEXT_PROBE_BYTES as i64);
    if n <= 0 {
        return Ok(());
    }
    let mut f = File::open(full).map_err(|e| Error::path("open", full, e))?;
    let mut probe = vec![0u8; n as usize];
    let got = read_full(&mut f, &mut probe).map_err(|e| Error::path("read", full, e))?;
    if is_binary(&probe[..got], size > got as i64) {
        return Err(Error::msg("binary or non-UTF-8 file"));
    }
    Ok(())
}

fn is_binary(b: &[u8], truncated: bool) -> bool {
    if b.contains(&0) {
        return true;
    }
    let b = if truncated {
        trim_incomplete_utf8(b)
    } else {
        b
    };
    std::str::from_utf8(b).is_err()
}

/// Drops a trailing partial rune so a size-capped probe does not fail UTF-8
/// validation when the cut falls mid-character.
fn trim_incomplete_utf8(b: &[u8]) -> &[u8] {
    if std::str::from_utf8(b).is_ok() {
        return b;
    }
    for i in 1..4 {
        if i > b.len() {
            break;
        }
        if std::str::from_utf8(&b[..b.len() - i]).is_ok() {
            return &b[..b.len() - i];
        }
    }
    b
}

/// Overwrites a file under `root`, creating parent dirs.
///
/// Atomic: this is the editor's save path, and a crash or full disk part-way
/// through a plain write would leave the source truncated. The content goes to
/// a synced sibling temp file renamed over the target, so a concurrent reader
/// sees the old file or the complete new one. The existing mode is kept
/// (an executable script stays executable); new files get 0644.
pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<()> {
    if content.len() > MAX_EDIT_BYTES {
        return Err(Error::msg(format!(
            "file too large to save ({} bytes)",
            content.len()
        )));
    }
    let full = safe_join_str(&path_str(root), rel)?;
    let dir = gopath::dir(&full);
    gopath::mkdir_all(&dir)?;
    let perm = fs::metadata(&full)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    let (tmp_name, mut tmp) = create_temp(&dir, &format!(".{}.tmp", gopath::base(&full)))?;
    let result = (|| {
        tmp.write_all(content.as_bytes())
            .map_err(|e| Error::path("write", &tmp_name, e))?;
        tmp.sync_all()
            .map_err(|e| Error::path("sync", &tmp_name, e))?;
        drop(tmp);
        fs::set_permissions(&tmp_name, fs::Permissions::from_mode(perm))
            .map_err(|e| Error::path("chmod", &tmp_name, e))?;
        fs::rename(&tmp_name, &full).map_err(|e| Error::path("rename", &format!("{tmp_name} {full}"), e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_name);
    }
    result
}

/// Go `os.CreateTemp(dir, prefix+"*")`: a new 0600 file with a random suffix.
fn create_temp(dir: &str, prefix: &str) -> Result<(String, File)> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut last = None;
    for _ in 0..10_000 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default()
            ^ (u64::from(std::process::id()) << 32)
            ^ SEQ.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let name = gopath::join(&[dir, &format!("{prefix}{}", seed % 1_000_000_000)]);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&name)
        {
            Ok(f) => return Ok((name, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(Error::path("open", &name, e)),
        }
    }
    Err(Error::path(
        "createtemp",
        &gopath::join(&[dir, &format!("{prefix}*")]),
        last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)),
    ))
}

/// The file's language for the editor, from the same cached lexer lookup as
/// diff highlighting.
pub(crate) fn language_for(path: &str) -> String {
    recur_git::lexer_name(&gopath::base(path))
}
