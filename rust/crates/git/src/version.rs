//! The git version check: Recur needs a git new enough for the flags it uses.

use std::fmt;
use std::time::Duration;

use tokio::sync::OnceCell;

use crate::command::command;
use crate::error::{Error, ErrorKind, Result};

/// The oldest supported git. `--diff-merges=first-parent` (commit list
/// numstat) landed in 2.31; older git fails on that flag and left the Commits
/// tab empty.
pub const MIN_VERSION: Version = Version {
    major: 2,
    minor: 31,
    patch: 0,
};

/// A git `X.Y.Z` (extra tokens like `Apple Git` or `windows.1` are ignored).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

impl Version {
    /// Whether `self` is strictly older than `o`.
    pub fn less(&self, o: &Version) -> bool {
        self < o
    }
}

/// Reads `git version` stdout (`git version 2.50.1 (Apple Git-155)`). A
/// non-numeric patch (`2.45.GIT`) counts as 0 so major.minor still compare.
pub fn parse_version(s: &str) -> Option<Version> {
    let mut s = s.trim();
    if let Some(i) = s.find("git version ") {
        s = &s[i + "git version ".len()..];
    }
    if let Some(i) = s.find([' ', '\t', '(']) {
        s = &s[..i];
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let major = leading_int(parts[0])?;
    let minor = leading_int(parts[1])?;
    let patch = parts.get(2).and_then(|p| leading_int(p)).unwrap_or(0);
    Some(Version {
        major,
        minor,
        patch,
    })
}

fn leading_int(s: &str) -> Option<i64> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    s[..n].parse().ok()
}

static PROBE: OnceCell<std::result::Result<Version, Error>> = OnceCell::const_new();

/// The version of the git on PATH. One subprocess per process.
pub async fn probe() -> Result<Version> {
    PROBE
        .get_or_init(|| async {
            let out = command(["version"])
                .timeout(Duration::from_secs(3))
                .output()
                .await
                .map_err(|e| e.wrap("git version"))?;
            let out = String::from_utf8_lossy(&out).into_owned();
            parse_version(&out).ok_or_else(|| {
                Error::with_kind(
                    format!("unparsed git version: {:?}", out.trim()),
                    ErrorKind::Unparsed,
                )
            })
        })
        .await
        .clone()
}

/// The user-facing alert when git is missing, unreadable, or older than
/// [`MIN_VERSION`]. Empty when the install is fine.
pub fn message_if_unsupported(probed: &Result<Version>) -> String {
    match probed {
        Err(e) if e.is_unparsed() => format!(
            "Recur needs Git {MIN_VERSION} or newer but could not read this Git's version. Update Git so pull-request commits load correctly."
        ),
        Err(_) => format!("Recur needs Git {MIN_VERSION} or newer on PATH. Git was not found."),
        Ok(v) if v.less(&MIN_VERSION) => format!(
            "Recur needs Git {MIN_VERSION} or newer (found {v}). Update Git so pull-request commits load correctly."
        ),
        Ok(_) => String::new(),
    }
}

/// The OS notification title and body for a failed probe; both empty when
/// git is fine.
pub fn unsupported_alert(probed: &Result<Version>) -> (String, String) {
    let body = message_if_unsupported(probed);
    if body.is_empty() {
        return (String::new(), String::new());
    }
    let title = match probed {
        Ok(_) => "Git is too old",
        Err(e) if e.is_unparsed() => "Git version unknown",
        Err(_) => "Git was not found",
    };
    (title.to_string(), body)
}

/// `--diff-merges=first-parent` when git is new enough; omitted on older git
/// so `git log` does not fail and empty the Commits tab.
pub(crate) fn log_numstat_flags(probed: &Result<Version>) -> &'static [&'static str] {
    match probed {
        Ok(v) if !v.less(&MIN_VERSION) => &["--diff-merges=first-parent"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: i64, minor: i64, patch: i64) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    fn not_found() -> Result<Version> {
        Err(Error::with_kind(
            "exec: \"git\": executable file not found in $PATH",
            ErrorKind::NotFound,
        ))
    }

    fn unparsed(s: &str) -> Result<Version> {
        Err(Error::with_kind(
            format!("unparsed git version: {:?}", s),
            ErrorKind::Unparsed,
        ))
    }

    #[test]
    fn parse_version_cases() {
        let cases = [
            ("git version 2.31.0", Some(v(2, 31, 0))),
            ("git version 2.50.1 (Apple Git-155)", Some(v(2, 50, 1))),
            ("git version 2.39.2.windows.1", Some(v(2, 39, 2))),
            ("git version 2.31", Some(v(2, 31, 0))),
            ("2.24.3", Some(v(2, 24, 3))),
            ("git version 2.45.GIT", Some(v(2, 45, 0))),
            ("git version 2.31.0-rc2", Some(v(2, 31, 0))),
            ("git version 1", None),
            ("", None),
            ("not a version", None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_version(input), want, "parse_version({input:?})");
        }
    }

    #[test]
    fn version_less() {
        assert!(v(2, 30, 2).less(&MIN_VERSION));
        assert!(!v(2, 31, 0).less(&MIN_VERSION));
        assert!(!v(2, 50, 1).less(&MIN_VERSION));
    }

    #[test]
    fn message_if_unsupported_cases() {
        assert_eq!(message_if_unsupported(&Ok(v(2, 50, 1))), "");
        let got = message_if_unsupported(&Ok(v(2, 24, 3)));
        assert!(got.contains("2.31") && got.contains("2.24.3"), "{got}");
        let got = message_if_unsupported(&not_found());
        assert!(got.contains("2.31") && got.contains("not found"), "{got}");
        let got = message_if_unsupported(&unparsed("bogus"));
        assert!(
            !got.contains("not found") && got.contains("could not read"),
            "{got}"
        );
    }

    #[test]
    fn log_numstat_flags_cases() {
        assert_eq!(
            log_numstat_flags(&Ok(v(2, 31, 0))),
            ["--diff-merges=first-parent"]
        );
        assert!(log_numstat_flags(&Ok(v(2, 30, 0))).is_empty());
        assert!(log_numstat_flags(&not_found()).is_empty());
        let parsed = parse_version("git version 2.45.GIT").ok_or_else(|| Error::msg("unparsed"));
        assert_eq!(log_numstat_flags(&parsed), ["--diff-merges=first-parent"]);
    }

    #[test]
    fn unsupported_alert_cases() {
        assert_eq!(
            unsupported_alert(&Ok(v(2, 50, 1))),
            (String::new(), String::new())
        );
        let (title, body) = unsupported_alert(&Ok(v(2, 24, 0)));
        assert_eq!(title, "Git is too old");
        assert!(!body.is_empty());
        let (title, body) = unsupported_alert(&not_found());
        assert_eq!(title, "Git was not found");
        assert!(body.contains("not found"));
        let (title, body) = unsupported_alert(&unparsed("nope"));
        assert_eq!(title, "Git version unknown");
        assert!(!body.contains("not found"));
    }

    #[tokio::test]
    async fn probe_reads_installed_git() {
        let v = probe()
            .await
            .unwrap_or_else(|e| panic!("probe() error = {e}"));
        assert!(v.major >= 2, "probe() = {v}, want major >= 2");
    }
}
