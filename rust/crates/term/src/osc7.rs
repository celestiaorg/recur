use std::path::Path;

const MAX_CARRY: usize = 8 << 10;
const MAX_URI: usize = 4 << 10;
const PREFIX: &[u8] = b"\x1b]7;";

/// Watches PTY output for OSC 7 (cwd) reports.
///
/// Many shells (fish, oh-my-zsh, shell-integration snippets) emit `ESC ] 7 ; file://host/path`
/// terminated by BEL or ST after `cd`. Those sequences are observed without stripping them (the
/// emulator ignores OSC 7), so the session's recorded cwd tracks the shell instead of staying at
/// the directory it started in.
#[derive(Default)]
pub(crate) struct Osc7Parser {
    carry: Vec<u8>,
}

impl Osc7Parser {
    /// Scans `input` for complete OSC 7 sequences and returns the last valid existing directory
    /// they named, or `None` when none completed this call.
    pub(crate) fn feed(&mut self, input: &[u8]) -> Option<String> {
        if self.carry.is_empty() && input.is_empty() {
            return None;
        }
        let data = if self.carry.is_empty() {
            input.to_vec()
        } else {
            let mut d = std::mem::take(&mut self.carry);
            d.extend_from_slice(input);
            d
        };
        let mut cwd = None;
        let mut i = 0;
        while i < data.len() {
            let Some(esc) = data[i..].iter().position(|&b| b == 0x1b) else {
                return cwd;
            };
            i += esc;
            match match_osc7(&data[i..]) {
                Osc7Match::Incomplete => {
                    let rest = &data[i..];
                    let rest = &rest[rest.len().saturating_sub(MAX_CARRY)..];
                    self.carry = rest.to_vec();
                    return cwd;
                }
                Osc7Match::None => i += 1,
                Osc7Match::Seq(n) => {
                    if let Some(path) = osc7_path(&data[i..i + n]) {
                        cwd = Some(path);
                    }
                    i += n;
                }
            }
        }
        cwd
    }
}

enum Osc7Match {
    Seq(usize),
    Incomplete,
    None,
}

fn match_osc7(data: &[u8]) -> Osc7Match {
    if data.len() < PREFIX.len() {
        return if PREFIX.starts_with(data) {
            Osc7Match::Incomplete
        } else {
            Osc7Match::None
        };
    }
    if &data[..PREFIX.len()] != PREFIX {
        return Osc7Match::None;
    }
    for i in PREFIX.len()..data.len() {
        if i - PREFIX.len() > MAX_URI {
            return Osc7Match::None;
        }
        match data[i] {
            0x07 => return Osc7Match::Seq(i + 1),
            0x1b => {
                if i + 1 >= data.len() {
                    return Osc7Match::Incomplete;
                }
                if data[i + 1] == b'\\' {
                    return Osc7Match::Seq(i + 2);
                }
                return Osc7Match::None;
            }
            _ => {}
        }
    }
    Osc7Match::Incomplete
}

fn osc7_path(seq: &[u8]) -> Option<String> {
    if seq.len() < 5 || !seq.starts_with(PREFIX) {
        return None;
    }
    let end = if seq.ends_with(b"\x07") {
        seq.len() - 1
    } else if seq.ends_with(b"\x1b\\") {
        seq.len() - 2
    } else {
        return None;
    };
    let uri = std::str::from_utf8(&seq[PREFIX.len()..end]).ok()?;
    let u = url::Url::parse(uri).ok()?;
    if !u.scheme().eq_ignore_ascii_case("file") {
        return None;
    }
    let host = match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => String::new(),
    };
    if !host_is_local(&host) {
        return None;
    }
    // Go's url.Parse rejects malformed escapes and yields a decoded path, which Recur then
    // unescapes once more.
    let path = path_unescape(u.path())?;
    if path.is_empty() {
        return None;
    }
    let path = path_unescape(&path).unwrap_or(path);
    Path::new(&path).is_dir().then_some(path)
}

fn path_unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            let hi = b.get(i + 1).copied().and_then(hex)?;
            let lo = b.get(i + 2).copied().and_then(hex)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn host_is_local(host: &str) -> bool {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    if matches!(
        host.to_ascii_lowercase().as_str(),
        "" | "localhost" | "127.0.0.1" | "::1"
    ) {
        return true;
    }
    nix::unistd::gethostname()
        .ok()
        .and_then(|n| n.into_string().ok())
        .is_some_and(|name| host.eq_ignore_ascii_case(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_str().unwrap().to_string();
        (d, p)
    }

    #[test]
    fn extracts_file_uri() {
        let (_d, dir) = tmp();
        let mut p = Osc7Parser::default();
        let input = format!("hello\x1b]7;file://{dir}\x07world");
        assert_eq!(p.feed(input.as_bytes()), Some(dir));
    }

    #[test]
    fn st_terminated_and_unescapes() {
        let (root, _) = tmp();
        let dir = root.path().join("has space");
        std::fs::create_dir(&dir).unwrap();
        let dir = dir.to_str().unwrap().to_string();
        let mut p = Osc7Parser::default();
        let seq = format!("\x1b]7;file://{}\x1b\\", dir.replace(' ', "%20"));
        assert_eq!(p.feed(seq.as_bytes()), Some(dir));
    }

    #[test]
    fn carries_split_sequence() {
        let (_d, dir) = tmp();
        let mut p = Osc7Parser::default();
        let seq = format!("\x1b]7;file://{dir}\x07");
        assert_eq!(p.feed(&seq.as_bytes()[..5]), None);
        assert_eq!(p.feed(&seq.as_bytes()[5..]), Some(dir));
    }

    #[test]
    fn ignores_missing_directory() {
        let mut p = Osc7Parser::default();
        assert_eq!(
            p.feed(b"\x1b]7;file:///definitely/not/a/real/recur/dir\x07"),
            None
        );
    }

    #[test]
    fn last_complete_wins() {
        let (_a, a) = tmp();
        let (_b, b) = tmp();
        let mut p = Osc7Parser::default();
        let input = format!("\x1b]7;file://{a}\x07\x1b]7;file://{b}\x07");
        assert_eq!(p.feed(input.as_bytes()), Some(b));
    }

    #[test]
    fn ignores_non_local_host() {
        let (_d, dir) = tmp();
        let mut p = Osc7Parser::default();
        let seq = format!("\x1b]7;file://evil.example{dir}\x07");
        assert_eq!(p.feed(seq.as_bytes()), None);
    }

    #[test]
    fn accepts_localhost_and_hostname() {
        let (_d, dir) = tmp();
        let mut p = Osc7Parser::default();
        let seq = format!("\x1b]7;file://localhost{dir}\x07");
        assert_eq!(p.feed(seq.as_bytes()), Some(dir.clone()));
        let host = nix::unistd::gethostname().unwrap().into_string().unwrap();
        let mut p = Osc7Parser::default();
        let seq = format!("\x1b]7;file://{host}{dir}\x07");
        assert_eq!(p.feed(seq.as_bytes()), Some(dir), "hostname {host}");
    }
}
