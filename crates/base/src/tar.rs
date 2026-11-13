//! Tar archives: a streaming reader for ustar, GNU and pax archives (regular files,
//! directories and symlinks, with GNU long names and pax `path` / `linkpath` / `size`), and a
//! ustar writer for tests and fixtures.

use std::io::{self, Read, Write};

const BLOCK: usize = 512;
/// Cap on a GNU long name or pax header's data, read into memory.
const MAX_META: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// Hard links, devices, FIFOs and anything else.
    Other,
}

pub struct Archive<R> {
    inner: R,
    /// Unread data bytes of the current entry, then its padding to the block.
    remaining: u64,
    pad: u64,
    done: bool,
}

pub struct Entry<'a, R> {
    pub kind: Kind,
    pub path: String,
    pub link: String,
    pub mode: u32,
    archive: &'a mut Archive<R>,
}

impl<R: Read> Archive<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            remaining: 0,
            pad: 0,
            done: false,
        }
    }

    /// The next entry, skipping what the previous one left unread. `None` at the end-of-archive
    /// block or a clean EOF between entries.
    pub fn next_entry(&mut self) -> io::Result<Option<Entry<'_, R>>> {
        self.skip(self.remaining + self.pad)?;
        self.remaining = 0;
        self.pad = 0;
        let (mut long_path, mut long_link, mut pax_size) = (None, None, None);
        let mut h = [0u8; BLOCK];
        loop {
            if self.done || !self.read_block(&mut h)? || h.iter().all(|&b| b == 0) {
                self.done = true;
                return Ok(None);
            }
            let stored = num(&h[148..156])?;
            let sum: u64 = h
                .iter()
                .enumerate()
                .map(|(i, &b)| u64::from(if (148..156).contains(&i) { b' ' } else { b }))
                .sum();
            if stored != sum {
                return Err(invalid("tar: header checksum mismatch"));
            }
            let size = pax_size.take().map_or_else(|| num(&h[124..136]), Ok)?;
            let pad = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
            match h[156] {
                b'L' => long_path = Some(cstr(&self.read_meta(size, pad)?)),
                b'K' => long_link = Some(cstr(&self.read_meta(size, pad)?)),
                b'x' => {
                    for (k, v) in pax_records(&self.read_meta(size, pad)?) {
                        match k {
                            b"path" => long_path = Some(lossy(v)),
                            b"linkpath" => long_link = Some(lossy(v)),
                            b"size" => pax_size = Some(num(v)?),
                            _ => {}
                        }
                    }
                }
                b'g' => self.skip(size + pad)?,
                t => {
                    let path = long_path.unwrap_or_else(|| {
                        let name = cstr(&h[0..100]);
                        let prefix = cstr(&h[345..500]);
                        if &h[257..263] == b"ustar\0" && !prefix.is_empty() {
                            format!("{prefix}/{name}")
                        } else {
                            name
                        }
                    });
                    self.remaining = size;
                    self.pad = pad;
                    return Ok(Some(Entry {
                        kind: match t {
                            b'0' | 0 => Kind::File,
                            b'5' => Kind::Dir,
                            b'2' => Kind::Symlink,
                            _ => Kind::Other,
                        },
                        path,
                        link: long_link.unwrap_or_else(|| cstr(&h[157..257])),
                        mode: u32::try_from(num(&h[100..108])?)
                            .map_err(|_| invalid("tar: mode out of range"))?,
                        archive: self,
                    }));
                }
            }
        }
    }

    /// Fills `buf`; false on EOF before its first byte.
    fn read_block(&mut self, buf: &mut [u8; BLOCK]) -> io::Result<bool> {
        let mut n = 0;
        while n < BLOCK {
            match self.inner.read(&mut buf[n..]) {
                Ok(0) if n == 0 => return Ok(false),
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    fn read_meta(&mut self, size: u64, pad: u64) -> io::Result<Vec<u8>> {
        if size > MAX_META {
            return Err(invalid("tar: extended header too large"));
        }
        let mut data = Vec::with_capacity(size as usize);
        (&mut self.inner).take(size).read_to_end(&mut data)?;
        if data.len() as u64 != size {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.skip(pad)?;
        Ok(data)
    }

    fn skip(&mut self, n: u64) -> io::Result<()> {
        if io::copy(&mut (&mut self.inner).take(n), &mut io::sink())? != n {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }
}

impl<R: Read> Read for Entry<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let a = &mut *self.archive;
        let max = usize::try_from(a.remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        if max == 0 {
            return Ok(0);
        }
        let n = a.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        a.remaining -= n as u64;
        Ok(n)
    }
}

/// An octal field (NUL or space terminated), or GNU base-256 when the high bit is set.
fn num(field: &[u8]) -> io::Result<u64> {
    let overflow = || invalid("tar: number out of range");
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return field.iter().enumerate().try_fold(0u64, |n, (i, &b)| {
            let b = if i == 0 { b & 0x7f } else { b };
            n.checked_mul(256)
                .and_then(|n| n.checked_add(u64::from(b)))
                .ok_or_else(overflow)
        });
    }
    let mut digits = field
        .iter()
        .skip_while(|&&b| b == b' ' || b == 0)
        .take_while(|&&b| b != b' ' && b != 0);
    digits.try_fold(0u64, |n, &b| {
        if !(b'0'..=b'7').contains(&b) {
            return Err(invalid("tar: bad number"));
        }
        n.checked_mul(8)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or_else(overflow)
    })
}

/// pax records: `<len> <key>=<value>\n`, `len` counting the whole record.
fn pax_records(mut data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    std::iter::from_fn(move || {
        let sp = data.iter().position(|&b| b == b' ')?;
        let len: usize = std::str::from_utf8(&data[..sp]).ok()?.parse().ok()?;
        let record = data.get(sp + 1..len)?.strip_suffix(b"\n")?;
        data = &data[len..];
        let eq = record.iter().position(|&b| b == b'=')?;
        Some((&record[..eq], &record[eq + 1..]))
    })
}

fn cstr(b: &[u8]) -> String {
    lossy(b.split(|&c| c == 0).next().unwrap_or_default())
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Writes ustar entries. Names and link targets are at most 100 bytes and are not checked
/// for `..` or absolute paths, so tests can build hostile archives.
pub struct Builder<W> {
    inner: W,
}

impl<W: Write> Builder<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    pub fn file(&mut self, path: &str, mode: u32, data: &[u8]) -> io::Result<()> {
        self.inner
            .write_all(&header(b'0', path, "", mode, data.len() as u64)?)?;
        self.inner.write_all(data)?;
        self.inner
            .write_all(&[0; BLOCK][..(BLOCK - data.len() % BLOCK) % BLOCK])
    }

    pub fn dir(&mut self, path: &str, mode: u32) -> io::Result<()> {
        self.inner.write_all(&header(b'5', path, "", mode, 0)?)
    }

    pub fn symlink(&mut self, path: &str, target: &str) -> io::Result<()> {
        self.inner.write_all(&header(b'2', path, target, 0o777, 0)?)
    }

    /// Writes the end-of-archive blocks.
    pub fn finish(mut self) -> io::Result<W> {
        self.inner.write_all(&[0; 2 * BLOCK])?;
        Ok(self.inner)
    }
}

fn header(kind: u8, path: &str, link: &str, mode: u32, size: u64) -> io::Result<[u8; BLOCK]> {
    if path.len() > 100 || link.len() > 100 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tar: name longer than 100 bytes",
        ));
    }
    let mut h = [0u8; BLOCK];
    h[..path.len()].copy_from_slice(path.as_bytes());
    h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[148..156].fill(b' ');
    h[156] = kind;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..265].copy_from_slice(b"ustar\x0000");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Listed = (Kind, String, String, u32, Vec<u8>);

    fn entries(tar: &[u8]) -> io::Result<Vec<Listed>> {
        let mut ar = Archive::new(tar);
        let mut out = Vec::new();
        while let Some(mut e) = ar.next_entry()? {
            let mut body = Vec::new();
            e.read_to_end(&mut body)?;
            out.push((e.kind, e.path.clone(), e.link.clone(), e.mode, body));
        }
        Ok(out)
    }

    /// A metadata entry (`L` or `x`) carrying `data`.
    fn meta(kind: u8, data: &[u8]) -> Vec<u8> {
        let mut b = Builder::new(Vec::new());
        b.file("meta", 0o644, data).unwrap();
        let mut v = b.inner;
        v[156] = kind;
        v[148..156].fill(b' ');
        let sum: u32 = v[..BLOCK].iter().map(|&b| u32::from(b)).sum();
        v[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        v
    }

    #[test]
    fn round_trips_files_dirs_and_symlinks() {
        let mut b = Builder::new(Vec::new());
        b.dir("app/", 0o755).unwrap();
        b.file("app/bin", 0o755, &[7; 600]).unwrap();
        b.file("app/empty", 0o644, b"").unwrap();
        b.symlink("app/link", "bin").unwrap();
        let tar = b.finish().unwrap();
        let got = entries(&tar).unwrap();
        assert_eq!(
            got,
            vec![
                (Kind::Dir, "app/".into(), "".into(), 0o755, vec![]),
                (Kind::File, "app/bin".into(), "".into(), 0o755, vec![7; 600]),
                (Kind::File, "app/empty".into(), "".into(), 0o644, vec![]),
                (
                    Kind::Symlink,
                    "app/link".into(),
                    "bin".into(),
                    0o777,
                    vec![]
                ),
            ]
        );
    }

    #[test]
    fn skips_unread_data_and_stops_at_clean_eof() {
        let mut b = Builder::new(Vec::new());
        b.file("a", 0o644, &[1; 1000]).unwrap();
        b.file("b", 0o644, b"bee").unwrap();
        let tar = b.inner;
        let mut ar = Archive::new(&tar[..]);
        assert_eq!(ar.next_entry().unwrap().unwrap().path, "a");
        let mut e = ar.next_entry().unwrap().unwrap();
        let mut s = String::new();
        e.read_to_string(&mut s).unwrap();
        assert_eq!((e.path.as_str(), s.as_str()), ("b", "bee"));
        assert!(ar.next_entry().unwrap().is_none());
    }

    #[test]
    fn long_names_from_gnu_and_pax() {
        let long = format!("{}/rg", "d".repeat(150));
        let mut tar = meta(b'L', format!("{long}\0").as_bytes());
        let mut b = Builder::new(Vec::new());
        b.file("short", 0o755, b"one").unwrap();
        tar.extend(b.inner);
        let rec = |k: &str, v: &str| {
            let body = format!(" {k}={v}\n");
            let mut len = body.len();
            loop {
                let r = format!("{len}{body}");
                if r.len() == len {
                    return r;
                }
                len = r.len();
            }
        };
        let pax = rec("path", &long) + &rec("linkpath", "target") + &rec("SCHILY.xattr.x", "y");
        tar.extend(meta(b'x', pax.as_bytes()));
        let mut b = Builder::new(Vec::new());
        b.symlink("short", "").unwrap();
        tar.extend(b.finish().unwrap());
        let got = entries(&tar).unwrap();
        assert_eq!(got[0].1, long);
        assert_eq!(got[0].4, b"one");
        assert_eq!(
            (got[1].0, got[1].1.as_str()),
            (Kind::Symlink, long.as_str())
        );
        assert_eq!(got[1].2, "target");
    }

    #[test]
    fn rejects_corrupt_headers() {
        let mut b = Builder::new(Vec::new());
        b.file("a", 0o644, b"x").unwrap();
        let mut tar = b.finish().unwrap();
        tar[0] = b'b';
        assert_eq!(
            entries(&tar).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut b = Builder::new(Vec::new());
        b.file("a", 0o644, &[1; 100]).unwrap();
        let tar = &b.inner[..BLOCK + 10];
        assert_eq!(
            entries(tar).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn numbers_octal_and_base256() {
        assert_eq!(num(b"0000644\0").unwrap(), 0o644);
        assert_eq!(num(b"  755 \0").unwrap(), 0o755);
        assert_eq!(num(b"\0\0\0").unwrap(), 0);
        assert_eq!(
            num(&[0x80, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]).unwrap(),
            1 << 32
        );
        assert!(num(b"0009").is_err());
    }
}
