//! Zip archives: a reader for stored and deflated members (zip64 included, CRC checked) and a
//! small writer for tests and fixtures.

use flate2::Crc;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{self, Read, Seek, SeekFrom, Write};

const EOCD: u32 = 0x0605_4b50;
const EOCD64: u32 = 0x0606_4b50;
const EOCD64_LOCATOR: u32 = 0x0706_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;
/// The end record plus the longest comment.
const MAX_TAIL: u64 = 22 + 0xffff;

pub struct Archive<R> {
    inner: R,
    entries: Vec<Entry>,
}

pub struct Entry {
    pub name: String,
    /// Unix mode bits (type and permissions), or 0 when the archive was not made on Unix.
    pub mode: u32,
    /// Uncompressed size.
    pub size: u64,
    compressed: u64,
    method: u16,
    crc: u32,
    offset: u64,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

impl<R: Read + Seek> Archive<R> {
    /// Reads the central directory.
    pub fn new(mut inner: R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        let start = len.saturating_sub(MAX_TAIL);
        inner.seek(SeekFrom::Start(start))?;
        let mut tail = Vec::new();
        inner.read_to_end(&mut tail)?;
        let at = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|&i| u32_at(&tail, i) == EOCD)
            .ok_or_else(|| invalid("zip: no end of central directory"))?;
        let e = &tail[at..];
        let (mut count, mut cd_size, mut cd_offset) = (
            u64::from(u16_at(e, 10)),
            u64::from(u32_at(e, 12)),
            u64::from(u32_at(e, 16)),
        );
        if at >= 20 && u32_at(&tail, at - 20) == EOCD64_LOCATOR {
            inner.seek(SeekFrom::Start(u64_at(&tail, at - 12)))?;
            let mut e = [0u8; 56];
            inner.read_exact(&mut e)?;
            if u32_at(&e, 0) != EOCD64 {
                return Err(invalid("zip: bad zip64 end record"));
            }
            (count, cd_size, cd_offset) = (u64_at(&e, 32), u64_at(&e, 40), u64_at(&e, 48));
        }
        if cd_offset.checked_add(cd_size).is_none_or(|end| end > len) {
            return Err(invalid("zip: central directory out of bounds"));
        }
        inner.seek(SeekFrom::Start(cd_offset))?;
        let mut cd = vec![0u8; cd_size as usize];
        inner.read_exact(&mut cd)?;
        let mut entries = Vec::new();
        let mut p = 0;
        while entries.len() as u64 != count {
            let h = cd
                .get(p..p + 46)
                .ok_or_else(|| invalid("zip: truncated directory"))?;
            if u32_at(h, 0) != CENTRAL {
                return Err(invalid("zip: bad directory entry"));
            }
            let (n, x, c) = (
                usize::from(u16_at(h, 28)),
                usize::from(u16_at(h, 30)),
                usize::from(u16_at(h, 32)),
            );
            let name = cd.get(p + 46..p + 46 + n);
            let extra = cd.get(p + 46 + n..p + 46 + n + x);
            let (Some(name), Some(extra)) = (name, extra) else {
                return Err(invalid("zip: truncated directory"));
            };
            if u16_at(h, 8) & 1 != 0 {
                return Err(invalid("zip: encrypted entries are not supported"));
            }
            let mut size = u64::from(u32_at(h, 24));
            let mut compressed = u64::from(u32_at(h, 20));
            let mut offset = u64::from(u32_at(h, 42));
            zip64_fields(extra, [&mut size, &mut compressed, &mut offset]);
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                mode: if u16_at(h, 4) >> 8 == 3 {
                    u32_at(h, 38) >> 16
                } else {
                    0
                },
                size,
                compressed,
                method: u16_at(h, 10),
                crc: u32_at(h, 16),
                offset,
            });
            p += 46 + n + x + c;
        }
        Ok(Self { inner, entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The contents of entry `i`. Reading past its size, or a CRC mismatch at the end, is an
    /// error.
    pub fn open(&mut self, i: usize) -> io::Result<impl Read + '_> {
        let e = self
            .entries
            .get(i)
            .ok_or_else(|| invalid("zip: no such entry"))?;
        self.inner.seek(SeekFrom::Start(e.offset))?;
        let mut h = [0u8; 30];
        self.inner.read_exact(&mut h)?;
        if u32_at(&h, 0) != LOCAL {
            return Err(invalid("zip: bad local header"));
        }
        let skip = i64::from(u16_at(&h, 26)) + i64::from(u16_at(&h, 28));
        self.inner.seek(SeekFrom::Current(skip))?;
        let raw = (&mut self.inner).take(e.compressed);
        let data: Box<dyn Read + '_> = match e.method {
            0 => Box::new(raw),
            8 => Box::new(DeflateDecoder::new(raw)),
            m => {
                return Err(invalid(&format!(
                    "zip: compression method {m} not supported"
                )));
            }
        };
        Ok(Checked {
            data: data.take(e.size),
            crc: Crc::new(),
            got: 0,
            want_crc: e.crc,
            want_size: e.size,
        })
    }
}

/// Fills the zip64 extended-information fields (`0x0001`) whose 32-bit values were saturated,
/// in their fixed order: size, compressed size, local header offset.
fn zip64_fields(mut extra: &[u8], fields: [&mut u64; 3]) {
    while extra.len() >= 4 {
        let (id, len) = (u16_at(extra, 0), usize::from(u16_at(extra, 2)));
        let Some(body) = extra.get(4..4 + len) else {
            return;
        };
        if id == 1 {
            let mut at = 0;
            for f in fields {
                if *f == 0xffff_ffff && at + 8 <= body.len() {
                    *f = u64_at(body, at);
                    at += 8;
                }
            }
            return;
        }
        extra = &extra[4 + len..];
    }
}

struct Checked<R> {
    data: io::Take<R>,
    crc: Crc,
    got: u64,
    want_crc: u32,
    want_size: u64,
}

impl<R: Read> Read for Checked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.data.read(buf)?;
        self.crc.update(&buf[..n]);
        self.got += n as u64;
        if n == 0 && !buf.is_empty() {
            if self.got != self.want_size {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if self.crc.sum() != self.want_crc {
                return Err(invalid("zip: CRC mismatch"));
            }
        }
        Ok(n)
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or_default())
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default())
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Writes deflated zip members with Unix modes (no zip64), for tests and fixtures.
pub struct Builder<W> {
    inner: W,
    at: u64,
    central: Vec<u8>,
    count: u16,
}

impl<W: Write> Builder<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            at: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    /// A regular file; `mode` holds the permission bits.
    pub fn file(&mut self, name: &str, mode: u32, data: &[u8]) -> io::Result<()> {
        self.add(name, 0o100000 | mode, data)
    }

    pub fn dir(&mut self, name: &str, mode: u32) -> io::Result<()> {
        self.add(name, 0o040000 | mode, b"")
    }

    pub fn symlink(&mut self, name: &str, target: &str) -> io::Result<()> {
        self.add(name, 0o120777, target.as_bytes())
    }

    fn add(&mut self, name: &str, mode: u32, data: &[u8]) -> io::Result<()> {
        let mut enc = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data)?;
        let body = enc.finish()?;
        let mut crc = Crc::new();
        crc.update(data);
        let n = u16::try_from(name.len()).map_err(|_| invalid("zip: name too long"))?;
        let too_big = || invalid("zip: entry too large");
        let size = u32::try_from(data.len()).map_err(|_| too_big())?;
        let packed = u32::try_from(body.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(self.at).map_err(|_| too_big())?;
        // version needed, flags, method, time, date, crc, sizes, name length.
        let common = [
            &20u16.to_le_bytes()[..],
            &0u16.to_le_bytes(),
            &8u16.to_le_bytes(),
            &0u32.to_le_bytes(),
            &crc.sum().to_le_bytes(),
            &packed.to_le_bytes(),
            &size.to_le_bytes(),
            &n.to_le_bytes(),
        ]
        .concat();
        let local = [
            &LOCAL.to_le_bytes()[..],
            &common,
            &0u16.to_le_bytes(),
            name.as_bytes(),
        ]
        .concat();
        self.inner.write_all(&local)?;
        self.inner.write_all(&body)?;
        self.at += (local.len() + body.len()) as u64;
        self.central.extend_from_slice(
            &[
                &CENTRAL.to_le_bytes()[..],
                &(0x0300u16 | 20).to_le_bytes(),
                &common,
                &[0; 8], // extra, comment, disk, internal attributes
                &(mode << 16).to_le_bytes(),
                &offset.to_le_bytes(),
                name.as_bytes(),
            ]
            .concat(),
        );
        self.count += 1;
        Ok(())
    }

    /// Writes the central directory.
    pub fn finish(mut self) -> io::Result<W> {
        let too_big = || invalid("zip: archive too large");
        let size = u32::try_from(self.central.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(self.at).map_err(|_| too_big())?;
        self.inner.write_all(&self.central)?;
        self.inner.write_all(
            &[
                &EOCD.to_le_bytes()[..],
                &[0; 4],
                &self.count.to_le_bytes(),
                &self.count.to_le_bytes(),
                &size.to_le_bytes(),
                &offset.to_le_bytes(),
                &[0; 2],
            ]
            .concat(),
        )?;
        Ok(self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample() -> Vec<u8> {
        let mut b = Builder::new(Vec::new());
        b.dir("app/", 0o755).unwrap();
        b.file("app/bin", 0o755, &[9; 5000]).unwrap();
        b.file("app/empty", 0o644, b"").unwrap();
        b.symlink("app/link", "bin").unwrap();
        b.finish().unwrap()
    }

    fn read(zip: &[u8], i: usize) -> io::Result<Vec<u8>> {
        let mut ar = Archive::new(Cursor::new(zip))?;
        let mut out = Vec::new();
        ar.open(i)?.read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn round_trips_entries() {
        let zip = sample();
        let ar = Archive::new(Cursor::new(&zip)).unwrap();
        let got: Vec<_> = ar
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e.mode, e.size, e.is_dir()))
            .collect();
        assert_eq!(
            got,
            [
                ("app/", 0o040755, 0, true),
                ("app/bin", 0o100755, 5000, false),
                ("app/empty", 0o100644, 0, false),
                ("app/link", 0o120777, 3, false),
            ]
        );
        assert_eq!(read(&zip, 1).unwrap(), [9; 5000]);
        assert_eq!(read(&zip, 2).unwrap(), b"");
        assert_eq!(read(&zip, 3).unwrap(), b"bin");
    }

    #[test]
    fn finds_the_end_record_behind_a_comment() {
        let mut zip = sample();
        let len = zip.len();
        zip[len - 2..].copy_from_slice(&5u16.to_le_bytes());
        zip.extend_from_slice(b"hello");
        assert_eq!(read(&zip, 3).unwrap(), b"bin");
    }

    #[test]
    fn rejects_corruption() {
        let zip = sample();
        assert!(Archive::new(Cursor::new(&zip[..zip.len() - 30])).is_err());
        assert!(Archive::new(Cursor::new(b"not a zip")).is_err());
        let mut bad = zip.clone();
        // Flip the stored CRC of `app/link` in the central directory.
        let cd = bad
            .windows(4)
            .rposition(|w| w == CENTRAL.to_le_bytes())
            .unwrap();
        bad[cd + 16] ^= 0xff;
        let err = read(&bad, 3).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
    }
}
