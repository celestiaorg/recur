//! The parts of Go's `net/url.Parse` that compatible-provider base URL
//! validation depends on: which inputs fail to parse, and the resulting
//! scheme and host. Mirrors Go 1.26 with the module's `urlstrictcolons=0`
//! default (the port is everything after the last colon).

use std::net::Ipv6Addr;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Url {
    /// Lower-cased.
    pub scheme: String,
    pub host: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Host,
    Zone,
    Other,
}

/// Go `url.Parse`: `None` where Go returns an error.
pub(crate) fn parse(raw: &str) -> Option<Url> {
    let (u, frag) = raw.split_once('#').unwrap_or((raw, ""));
    let url = parse_no_frag(u)?;
    if !frag.is_empty() {
        unescape(frag, Mode::Other)?;
    }
    Some(url)
}

fn parse_no_frag(raw: &str) -> Option<Url> {
    if raw.bytes().any(|b| b < b' ' || b == 0x7f) {
        return None;
    }
    if raw == "*" {
        return Some(Url::default());
    }
    let (scheme, rest) = get_scheme(raw)?;
    let scheme = scheme.to_lowercase();
    let rest = if rest.ends_with('?') && rest.matches('?').count() == 1 {
        &rest[..rest.len() - 1]
    } else {
        rest.split_once('?').map_or(rest, |(r, _)| r)
    };
    if !rest.starts_with('/') {
        if !scheme.is_empty() {
            // Rootless paths are opaque: no host.
            return Some(Url {
                scheme,
                host: String::new(),
            });
        }
        let segment = rest.split_once('/').map_or(rest, |(s, _)| s);
        if segment.contains(':') {
            return None;
        }
    }
    let mut host = String::new();
    let mut path = rest;
    if (!scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
        let after = &rest[2..];
        let (authority, p) = after.find('/').map_or((after, ""), |i| after.split_at(i));
        host = parse_authority(authority)?;
        path = p;
    }
    unescape(path, Mode::Other)?;
    Some(Url { scheme, host })
}

fn get_scheme(raw: &str) -> Option<(&str, &str)> {
    for (i, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if i > 0 => {}
            b':' if i == 0 => return None, // missing protocol scheme
            b':' => return Some((&raw[..i], &raw[i + 1..])),
            _ => return Some(("", raw)),
        }
    }
    Some(("", raw))
}

fn parse_authority(authority: &str) -> Option<String> {
    let at = authority.rfind('@');
    let host = parse_host(at.map_or(authority, |i| &authority[i + 1..]))?;
    if let Some(i) = at {
        let userinfo = &authority[..i];
        if !userinfo.chars().all(valid_userinfo_char) {
            return None;
        }
        let (user, pass) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        unescape(user, Mode::Other)?;
        unescape(pass, Mode::Other)?;
    }
    Some(host)
}

fn valid_userinfo_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._:~!$&'()*+,;=%@".contains(c)
}

fn parse_host(host: &str) -> Option<String> {
    match host.rfind('[') {
        Some(i) if i > 0 => None, // invalid IP-literal
        Some(_) => {
            let close = host.rfind(']')?;
            let colon_port = &host[close + 1..];
            if !valid_optional_port(colon_port) {
                return None;
            }
            let hostname = &host[1..close];
            let unescaped = match hostname.find("%25") {
                Some(z) => {
                    unescape(&hostname[..z], Mode::Host)? + &unescape(&hostname[z..], Mode::Zone)?
                }
                None => unescape(hostname, Mode::Host)?,
            };
            // Only a valid IPv6 address may be bracketed (zones allowed).
            let addr = match unescaped.split_once('%') {
                Some((_, "")) => return None,
                Some((addr, _)) => addr,
                None => &unescaped,
            };
            addr.parse::<Ipv6Addr>().ok()?;
            Some(format!("[{unescaped}]{colon_port}"))
        }
        None => {
            if let Some(i) = host.rfind(':')
                && !valid_optional_port(&host[i..])
            {
                return None;
            }
            unescape(host, Mode::Host)
        }
    }
}

fn valid_optional_port(port: &str) -> bool {
    port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit()))
}

/// Bytes Go's escape table allows unescaped in a host or zone.
fn host_byte_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!\"$&'()*+,-.:;<=>[]_~".contains(&b)
}

fn unhex(c: u8) -> u8 {
    9 * (c >> 6) + (c & 15)
}

fn unescape(s: &str, mode: Mode) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if i + 2 >= b.len()
                    || !b[i + 1].is_ascii_hexdigit()
                    || !b[i + 2].is_ascii_hexdigit()
                {
                    return None;
                }
                let is_pct25 = &b[i..i + 3] == b"%25";
                let v = (unhex(b[i + 1]) << 4) | unhex(b[i + 2]);
                // In a host, %-encoding is only for non-ASCII bytes (and %25).
                if mode == Mode::Host && unhex(b[i + 1]) < 8 && !is_pct25 {
                    return None;
                }
                if mode == Mode::Zone && !is_pct25 && v != b' ' && !host_byte_ok(v) {
                    return None;
                }
                out.push(v);
                i += 3;
            }
            c => {
                if mode != Mode::Other && c < 0x80 && !host_byte_ok(c) {
                    return None; // invalid character in host name
                }
                out.push(c);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> (String, String) {
        let u = parse(raw).unwrap_or_else(|| panic!("{raw:?} should parse"));
        (u.scheme, u.host)
    }

    #[test]
    fn hosts_and_schemes() {
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(ok("https://api.openai.com"), s("https", "api.openai.com"));
        assert_eq!(
            ok("HTTP://Example.com:8080/v1?x=1#f"),
            s("http", "Example.com:8080")
        );
        assert_eq!(ok("http://user:p@ss@host/x"), s("http", "host"));
        assert_eq!(ok("http://[::1]:11434"), s("http", "[::1]:11434"));
        assert_eq!(ok("http://[fe80::1%25en0]"), s("http", "[fe80::1%en0]"));
        assert_eq!(ok("http://h%C3%A9.com"), s("http", "hé.com"));
        assert_eq!(ok("localhost:8080"), s("localhost", ""), "opaque");
        assert_eq!(ok("http:example.com"), s("http", ""));
        assert_eq!(ok("//host/x"), s("", "host"));
        assert_eq!(ok("example.com/v1"), s("", ""));
        assert_eq!(ok("http://"), s("http", ""));
        assert_eq!(ok("http://a:1:2"), s("http", "a:1:2"), "lenient colons");
        assert_eq!(ok("*"), s("", ""));
    }

    #[test]
    fn rejects() {
        for raw in [
            ":foo",
            "http://exa mple.com",
            "http://host:abc",
            "http://[::1",
            "http://[1.2.3.4]",
            "http://x[::1]",
            "http://[fe80::1%25]",
            "http://h%2f.com",
            "http://host/%zz",
            "http://host/#%z",
            "http://a b@host",
            "http://host\n",
            "1a:b/c",
            "http://h^st",
        ] {
            assert_eq!(parse(raw), None, "{raw:?}");
        }
    }
}
