//! A line scanner with a 1 MiB token cap: lines lose their `\n` and a trailing
//! `\r`, and a longer line stops the scan with `bufio.Scanner: token too long`.

use std::io::{self, BufRead, BufReader, Read};
use std::ops::ControlFlow;

const MAX_TOKEN: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) enum ScanError {
    TooLong,
    Io(io::Error),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => f.write_str("bufio.Scanner: token too long"),
            Self::Io(e) => write!(f, "{}", e),
        }
    }
}

/// Feeds each line to `f` with its 1-based number until `f` breaks.
pub(crate) fn scan_lines<B>(
    r: impl Read,
    mut f: impl FnMut(usize, &[u8]) -> ControlFlow<B>,
) -> Result<Option<B>, ScanError> {
    let mut r = BufReader::with_capacity(64 * 1024, r);
    let mut line = Vec::new();
    let mut n = 0;
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ScanError::Io(e)),
        };
        if buf.is_empty() {
            if !line.is_empty() {
                n += 1;
                if let ControlFlow::Break(b) = f(n, drop_cr(&line)) {
                    return Ok(Some(b));
                }
            }
            return Ok(None);
        }
        match buf.iter().position(|&c| c == b'\n') {
            Some(i) => {
                line.extend_from_slice(&buf[..i]);
                r.consume(i + 1);
                if line.len() >= MAX_TOKEN {
                    return Err(ScanError::TooLong);
                }
                n += 1;
                if let ControlFlow::Break(b) = f(n, drop_cr(&line)) {
                    return Ok(Some(b));
                }
                line.clear();
            }
            None => {
                let len = buf.len();
                line.extend_from_slice(buf);
                r.consume(len);
                if line.len() >= MAX_TOKEN {
                    return Err(ScanError::TooLong);
                }
            }
        }
    }
}

fn drop_cr(b: &[u8]) -> &[u8] {
    b.strip_suffix(b"\r").unwrap_or(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        scan_lines::<()>(input, |_, l| {
            out.push(String::from_utf8_lossy(l).into_owned());
            ControlFlow::Continue(())
        })
        .map_err(|e| e.to_string())?;
        Ok(out)
    }

    #[test]
    fn like_bufio() {
        assert_eq!(lines(b"a\r\nb\n\nc").unwrap(), ["a", "b", "", "c"]);
        assert_eq!(lines(b"a\n").unwrap(), ["a"]);
        assert!(lines(b"").unwrap().is_empty());
        let long = vec![b'x'; MAX_TOKEN + 10];
        assert_eq!(lines(&long).unwrap_err(), "bufio.Scanner: token too long");
    }
}
