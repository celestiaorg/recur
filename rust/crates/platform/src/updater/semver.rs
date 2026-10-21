//! SemVer 2.0.0 precedence with `golang.org/x/mod/semver` semantics: `v1`
//! and `v1.2` shorthands are valid, build metadata is ignored, and invalid
//! strings are equal to each other and lower than every valid version.

use std::cmp::Ordering;

/// Strips one leading `v` or `V`.
pub fn trim_prefix(tag: &str) -> &str {
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

/// Compares two versions; either may carry the leading `v`.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (parse(&canonical(a)), parse(&canonical(b))) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => compare_int(x.major, y.major)
            .then_with(|| compare_int(x.minor, y.minor))
            .then_with(|| compare_int(x.patch, y.patch))
            .then_with(|| compare_prerelease(x.prerelease, y.prerelease)),
    }
}

/// Whether `tag` is strictly newer than `current`. False for an empty tag;
/// true for any tag when `current` is empty.
pub fn is_newer(tag: &str, current: &str) -> bool {
    if trim_prefix(tag).is_empty() {
        return false;
    }
    if trim_prefix(current).is_empty() {
        return true;
    }
    compare(tag, current) == Ordering::Greater
}

fn canonical(s: &str) -> String {
    let s = trim_prefix(s);
    if s.is_empty() {
        String::new()
    } else {
        format!("v{s}")
    }
}

struct Parsed<'a> {
    major: &'a str,
    minor: &'a str,
    patch: &'a str,
    prerelease: &'a str,
}

fn parse(v: &str) -> Option<Parsed<'_>> {
    let v = v.strip_prefix('v')?;
    let (major, v) = parse_int(v)?;
    if v.is_empty() {
        return Some(Parsed {
            major,
            minor: "0",
            patch: "0",
            prerelease: "",
        });
    }
    let (minor, v) = parse_int(v.strip_prefix('.')?)?;
    if v.is_empty() {
        return Some(Parsed {
            major,
            minor,
            patch: "0",
            prerelease: "",
        });
    }
    let (patch, mut v) = parse_int(v.strip_prefix('.')?)?;
    let mut prerelease = "";
    if v.starts_with('-') {
        (prerelease, v) = parse_prerelease(v)?;
    }
    if v.starts_with('+') {
        v = parse_build(v)?;
    }
    v.is_empty().then_some(Parsed {
        major,
        minor,
        patch,
        prerelease,
    })
}

fn parse_int(v: &str) -> Option<(&str, &str)> {
    let n = v.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 || (v.starts_with('0') && n != 1) {
        return None;
    }
    Some(v.split_at(n))
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}

fn is_bad_num(v: &str) -> bool {
    v.len() > 1 && v.starts_with('0') && v.bytes().all(|c| c.is_ascii_digit())
}

fn is_num(v: &str) -> bool {
    v.bytes().all(|c| c.is_ascii_digit())
}

fn parse_prerelease(v: &str) -> Option<(&str, &str)> {
    let b = v.as_bytes();
    let (mut i, mut start) = (1, 1);
    while i < b.len() && b[i] != b'+' {
        if !is_ident_char(b[i]) && b[i] != b'.' {
            return None;
        }
        if b[i] == b'.' {
            if start == i || is_bad_num(&v[start..i]) {
                return None;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start == i || is_bad_num(&v[start..i]) {
        return None;
    }
    Some(v.split_at(i))
}

fn parse_build(v: &str) -> Option<&str> {
    let b = v.as_bytes();
    let (mut i, mut start) = (1, 1);
    while i < b.len() {
        if !is_ident_char(b[i]) && b[i] != b'.' {
            return None;
        }
        if b[i] == b'.' {
            if start == i {
                return None;
            }
            start = i + 1;
        }
        i += 1;
    }
    (start != i).then_some(&v[i..])
}

fn compare_int(x: &str, y: &str) -> Ordering {
    x.len().cmp(&y.len()).then_with(|| x.cmp(y))
}

fn compare_prerelease(x: &str, y: &str) -> Ordering {
    if x == y {
        return Ordering::Equal;
    }
    if x.is_empty() {
        return Ordering::Greater;
    }
    if y.is_empty() {
        return Ordering::Less;
    }
    let mut xs = x[1..].split('.');
    let mut ys = y[1..].split('.');
    loop {
        match (xs.next(), ys.next()) {
            (Some(dx), Some(dy)) => {
                if dx == dy {
                    continue;
                }
                return match (is_num(dx), is_num(dy)) {
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (true, true) => compare_int(dx, dy),
                    (false, false) => dx.cmp(dy),
                };
            }
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (None, None) => return Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer() {
        let cases = [
            ("v2.0.0", "1.0.0", true),
            ("2.0.0", "v1.0.0", true),
            ("v1.0.0", "v1.0.0", false),
            ("v0.9.0", "v1.0.0", false),
            ("v1.10.0", "v1.2.0", true),
            ("v1.2.0", "v1.10.0", false),
            ("v1.2.3-rc.1", "v1.2.3", false),
            ("v1.2.3", "v1.2.3-rc.1", true),
            ("v1.2.3-rc.2", "v1.2.3-rc.1", true),
            ("v1.2.3-beta", "v1.2.3-alpha", true),
            ("v1.2.3+build.1", "v1.2.3", false),
            ("v1.2.3+build.2", "v1.2.3+build.1", false),
            ("", "1.0.0", false),
            ("1.0.0", "", true),
            ("v", "1.0.0", false),
        ];
        for (tag, current, want) in cases {
            assert_eq!(
                is_newer(tag, current),
                want,
                "is_newer({tag:?}, {current:?})"
            );
        }
    }

    #[test]
    fn trims_prefix() {
        for (input, want) in [
            ("v1.2.3", "1.2.3"),
            ("V1.2.3", "1.2.3"),
            ("1.2.3", "1.2.3"),
            ("", ""),
            ("v", ""),
        ] {
            assert_eq!(trim_prefix(input), want);
        }
    }

    #[test]
    fn precedence_chain_and_shorthands() {
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for w in chain.windows(2) {
            assert_eq!(compare(w[0], w[1]), Ordering::Less, "{} < {}", w[0], w[1]);
        }
        assert_eq!(compare("v1", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(
            compare("01.0.0", "bogus"),
            Ordering::Equal,
            "invalid == invalid"
        );
        assert_eq!(compare("bogus", "0.0.1"), Ordering::Less);
        assert_eq!(
            compare("1.0.0-01", "1.0.0"),
            Ordering::Less,
            "invalid prerelease is invalid"
        );
    }
}
