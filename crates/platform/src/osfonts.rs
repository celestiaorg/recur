//! Locally installed monospace font families for the settings font picker.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

/// Distinct font-family names that are monospace, sorted.
///
/// Sources (union):
///  1. fontconfig `fc-list :spacing=mono` (when available), minus known junk
///  2. A scan of common font dirs, keeping a face only if OpenType says
///     fixed-pitch (post.isFixedPitch / OS/2 PANOSE) or the family name is a
///     known mono
pub fn monospace_families() -> Vec<String> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut add = |name: &str| {
        let name = clean_family(name);
        if name.is_empty() || name.starts_with('.') || is_junk_mono_name(name) {
            return;
        }
        seen.entry(name.to_lowercase())
            .or_insert_with(|| name.to_string());
    };

    // fontconfig's spacing=mono is noisy on macOS (emoji, bitmaps...). Only
    // keep faces whose names clearly look like coding monos; the directory
    // scan is the OpenType-authoritative path.
    for n in from_fontconfig() {
        if is_junk_mono_name(&n) || !looks_mono_name(&n) {
            continue;
        }
        add(&n);
    }
    for n in from_font_dirs(&font_dirs()) {
        add(&n);
    }

    let mut out: Vec<String> = seen.into_values().collect();
    out.sort();
    out
}

fn clean_family(name: &str) -> &str {
    let name = name.trim();
    // fc-list can return comma-separated localized names; keep the first.
    match name.find(',') {
        Some(i) => name[..i].trim(),
        None => name,
    }
}

/// Families that are monospace even when the fixed-pitch bit is missing or
/// wrong. Kept specific: bare "code"/"fira" match proportional faces.
static MONO_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alts = [
        r".*\bmono\b.*",
        r".*\bconsolas\b.*",
        r"courier(\s+new)?$",
        r".*\binconsolata\b.*",
        r"menlo.*",
        r"monaco$",
        r".*\biosevka\b.*",
        r".*\bmeslo\b.*",
        r".*\bjetbrains\b.*",
        r"hack(\s+nerd\s+font|\s+nf)?$",
        r"fira\s*(code|mono).*",
        r"cascadia\s*(code|mono).*",
        r"source\s*code\s*pro.*",
        r"ibm\s*plex\s*mono.*",
        r"ubuntu\s*mono.*",
        r"dejavu\s*sans\s*mono.*",
        r"liberation\s*mono.*",
        r"noto\s*sans\s*mono.*",
        r"droid\s*sans\s*mono.*",
        r"anonymous\s*pro.*",
        r"pt\s*mono.*",
        r"andale\s*mono.*",
        r"commit\s*mono.*",
        r"geist\s*mono.*",
        r"input\s*mono.*",
        r"operator\s*mono.*",
        r"berkeley\s*mono.*",
        r"sauce\s*code.*",
        r"caskaydia\s*cove.*",
        r"press\s*start\s*2p$",
    ];
    Regex::new(&format!("(?i)^({})$", alts.join("|"))).expect("mono name regex")
});

fn looks_mono_name(family: &str) -> bool {
    MONO_NAME_RE.is_match(family)
}

/// Fontconfig on macOS tags some non-mono faces as spacing=mono.
fn is_junk_mono_name(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    [
        "emoji",
        "lastresort",
        "last resort",
        "bitmap",
        "braille",
        "ornament",
        "dingbat",
        "wingding",
        "symbol",
        "notodef",
        "apple color",
        "gb18030",
        "times lt mm",
        "multiple master",
    ]
    .iter()
    .any(|bad| lower.contains(bad))
}

fn from_fontconfig() -> Vec<String> {
    let Some(bin) = find_fc_list() else {
        return Vec::new();
    };
    let out = crate::run_with_timeout(
        Command::new(bin).args([":spacing=mono", "family"]),
        Duration::from_secs(3),
        false,
    );
    let Some(out) = out.filter(|o| o.success) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.output)
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn find_fc_list() -> Option<PathBuf> {
    if let Some(p) = crate::look_path("fc-list") {
        return Some(p);
    }
    for p in [
        "/opt/homebrew/bin/fc-list",
        "/usr/local/bin/fc-list",
        "/usr/bin/fc-list",
    ] {
        if Path::new(p).is_file() {
            return Some(PathBuf::from(p));
        }
    }
    let mut matches: Vec<PathBuf> = std::fs::read_dir("/opt/homebrew/Cellar/fontconfig")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("bin").join("fc-list"))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    matches.sort();
    matches.pop()
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::home_dir() {
        dirs.push(home.join("Library").join("Fonts"));
        dirs.push(home.join(".local").join("share").join("fonts"));
        dirs.push(home.join(".fonts"));
    }
    for d in [
        "/Library/Fonts",
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

fn from_font_dirs(dirs: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();
    for dir in dirs {
        walk_files(dir, &mut |path| {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf")) {
                return;
            }
            if let Ok((family, mono)) = inspect_font(path)
                && !family.is_empty()
                && (mono || looks_mono_name(&family))
            {
                names.push(family);
            }
        });
    }
    names
}

/// Lexical-order walk like `filepath.WalkDir`; symlinks are not followed and
/// unreadable entries are skipped.
fn walk_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() {
            walk_files(&path, visit);
        } else {
            visit(&path);
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum FontError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not a single sfnt font")]
    NotSfnt,
    #[error("table too large")]
    TableTooLarge,
    #[error("missing table")]
    MissingTable,
}

/// Reads the OpenType/TrueType `name` family and the fixed-pitch flag from
/// `post` (isFixedPitch) or the OS/2 PANOSE proportion.
fn inspect_font(path: &Path) -> Result<(String, bool), FontError> {
    let mut f = File::open(path)?;
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr)?;
    match &hdr[0..4] {
        b"\x00\x01\x00\x00" | b"OTTO" | b"true" | b"typ1" => {}
        _ => return Err(FontError::NotSfnt),
    }
    let num_tables = u16::from_be_bytes([hdr[4], hdr[5]]);
    let mut tables = Vec::with_capacity(num_tables as usize);
    for _ in 0..num_tables {
        let mut rec = [0u8; 16];
        f.read_exact(&mut rec)?;
        let tag: [u8; 4] = rec[0..4].try_into().expect("4 bytes");
        let offset = u32::from_be_bytes(rec[8..12].try_into().expect("4 bytes"));
        let length = u32::from_be_bytes(rec[12..16].try_into().expect("4 bytes"));
        tables.push((tag, offset, length));
    }

    let mut read_table = |tag: &[u8; 4]| -> Result<Vec<u8>, FontError> {
        let &(_, offset, length) = tables
            .iter()
            .find(|(t, _, _)| t == tag)
            .ok_or(FontError::MissingTable)?;
        if length > 4 << 20 {
            return Err(FontError::TableTooLarge);
        }
        let mut buf = vec![0u8; length as usize];
        f.seek(SeekFrom::Start(u64::from(offset)))?;
        f.read_exact(&mut buf)?;
        Ok(buf)
    };

    let family = read_table(b"name")
        .map(|d| family_from_name_table(&d))
        .unwrap_or_default();
    // 'post': version(4) + italicAngle(4) + underlinePosition(2) +
    // underlineThickness(2) + isFixedPitch(4), so isFixedPitch is at 12.
    let mut mono = matches!(read_table(b"post"), Ok(post) if post.len() >= 16
        && u32::from_be_bytes(post[12..16].try_into().expect("4 bytes")) != 0);
    // PANOSE starts at offset 32 in OS/2; proportion (index 3) == 9 is mono.
    if !mono && let Ok(os2) = read_table(b"OS/2") {
        mono = os2.len() >= 42 && os2[32 + 3] == 9;
    }
    Ok((family, mono))
}

fn family_from_name_table(data: &[u8]) -> String {
    if data.len() < 6 {
        return String::new();
    }
    let be16 = |off: usize| usize::from(u16::from_be_bytes([data[off], data[off + 1]]));
    let count = be16(2);
    let string_offset = be16(4);
    // Prefer the Windows English family (name ID 1), then Mac Roman.
    let rank = |platform, encoding, lang, name_id| -> i32 {
        let mut r = match name_id {
            1 => 100,
            16 => 90, // typographic family
            _ => return -1,
        };
        if platform == 3 && encoding == 1 && (lang == 0x409 || lang == 0) {
            r += 20;
        } else if platform == 1 && encoding == 0 {
            r += 10;
        } else if platform == 3 {
            r += 5;
        }
        r
    };
    let mut best = String::new();
    let mut best_rank = 0;
    for i in 0..count {
        let off = 6 + i * 12;
        if off + 12 > data.len() {
            break;
        }
        let platform = be16(off);
        let r = rank(platform, be16(off + 2), be16(off + 4), be16(off + 6));
        if r < 0 || r < best_rank {
            continue;
        }
        let start = string_offset + be16(off + 10);
        let end = start + be16(off + 8);
        if end > data.len() {
            continue;
        }
        let raw = &data[start..end];
        let s = if platform == 3 || platform == 0 {
            decode_utf16be(raw)
        } else {
            String::from_utf8_lossy(raw).into_owned()
        };
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        best = s.to_string();
        best_rank = r;
    }
    best
}

/// Each code unit becomes one char; unpaired or paired surrogates become
/// U+FFFD.
fn decode_utf16be(b: &[u8]) -> String {
    if !b.len().is_multiple_of(2) {
        return String::new();
    }
    b.chunks_exact(2)
        .map(|c| {
            char::from_u32(u32::from(u16::from_be_bytes([c[0], c[1]])))
                .unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_font(name: &str) -> Option<PathBuf> {
        let p = crate::home_dir()?.join("Library").join("Fonts").join(name);
        p.exists().then_some(p)
    }

    #[test]
    fn includes_inconsolata_when_present() {
        if !cfg!(target_os = "macos") {
            return;
        }
        if home_font("Inconsolata-Regular.ttf").is_none() {
            return;
        }
        let fams = monospace_families();
        assert!(
            fams.iter().any(|f| f.eq_ignore_ascii_case("Inconsolata")),
            "expected Inconsolata in monospace list, got {fams:?}"
        );
    }

    #[test]
    fn inspect_inconsolata() {
        let Some(path) = home_font("Inconsolata-Regular.ttf") else {
            return;
        };
        let (family, mono) = inspect_font(&path).expect("inspect");
        assert!(
            family.eq_ignore_ascii_case("Inconsolata"),
            "family={family:?}"
        );
        assert!(mono, "expected post.isFixedPitch for Inconsolata");
    }

    #[test]
    fn inspect_arial_not_mono() {
        let Some(path) = [
            "/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.exists()) else {
            return;
        };
        let (_, mono) = inspect_font(&path).expect("inspect");
        assert!(!mono, "Arial should not be fixed-pitch");
    }

    #[test]
    fn excludes_proportional_faces() {
        for f in monospace_families() {
            assert!(
                !f.eq_ignore_ascii_case("Arial") && !f.eq_ignore_ascii_case("Comic Sans MS"),
                "proportional face {f:?} leaked into monospace list"
            );
        }
    }

    /// Builds a minimal sfnt with `name` (one record), `post` and `OS/2`.
    fn sfnt(family: &str, fixed_pitch: u32, panose_proportion: u8) -> Vec<u8> {
        let utf16: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut name = Vec::new();
        name.extend(0u16.to_be_bytes()); // format
        name.extend(1u16.to_be_bytes()); // count
        name.extend(18u16.to_be_bytes()); // string offset (6 + 12)
        for v in [3u16, 1, 0x409, 1, utf16.len() as u16, 0] {
            name.extend(v.to_be_bytes());
        }
        name.extend(&utf16);
        let mut post = vec![0u8; 32];
        post[12..16].copy_from_slice(&fixed_pitch.to_be_bytes());
        let mut os2 = vec![0u8; 96];
        os2[35] = panose_proportion;

        let tables: [(&[u8; 4], Vec<u8>); 3] = [(b"OS/2", os2), (b"name", name), (b"post", post)];
        let mut out = Vec::new();
        out.extend(b"\x00\x01\x00\x00");
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut body: Vec<u8> = Vec::new();
        for (tag, data) in &tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend((offset as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            offset += data.len();
            body.extend(data);
        }
        out.extend(body);
        out
    }

    #[test]
    fn inspect_synthetic_fonts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = [
            ("Fixed Sans", 1, 0, true),
            ("Panose Sans", 0, 9, true),
            ("Proportional Sans", 0, 2, false),
        ];
        for (family, fixed, panose, want_mono) in cases {
            let p = dir.path().join(format!("{family}.ttf"));
            std::fs::write(&p, sfnt(family, fixed, panose)).expect("write");
            let (got, mono) = inspect_font(&p).expect("inspect");
            assert_eq!(got, family);
            assert_eq!(mono, want_mono, "{family}");
        }
        let junk = dir.path().join("junk.ttf");
        std::fs::write(&junk, b"not a font at all").expect("write");
        assert!(inspect_font(&junk).is_err());

        let named = dir.path().join("JetBrains Mono.otf");
        std::fs::write(&named, sfnt("JetBrains Mono", 0, 2)).expect("write");
        let mut found = from_font_dirs(&[dir.path().to_path_buf()]);
        found.sort();
        assert_eq!(found, ["Fixed Sans", "JetBrains Mono", "Panose Sans"]);
    }

    #[test]
    fn mono_names_and_junk() {
        for n in [
            "Menlo",
            "Fira Code",
            "Hack Nerd Font",
            "Courier New",
            "Something Mono",
        ] {
            assert!(looks_mono_name(n), "{n}");
        }
        for n in ["Fira Sans", "Courier Prime", "Code Bold", "Hackney"] {
            assert!(!looks_mono_name(n), "{n}");
        }
        assert!(is_junk_mono_name("Apple Color Emoji"));
        assert!(!is_junk_mono_name("Menlo"));
        assert_eq!(
            clean_family("  Noto Sans Mono,Noto Sans Mono CJK "),
            "Noto Sans Mono"
        );
    }

    #[test]
    fn utf16_decoding() {
        assert_eq!(decode_utf16be(&[0, b'A', 0, b'b']), "Ab");
        assert_eq!(decode_utf16be(&[0, b'A', 0]), "");
        assert_eq!(decode_utf16be(&[0xD8, 0x3D]), "\u{FFFD}");
    }
}
