//! In-tree replacements for small external crates, cut down to what Recur uses.

pub mod embed;
pub mod tar;
pub mod tz;
pub mod zip;

/// Lowercase hex of `bytes`.
pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 0xf)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn hex_encodes_lowercase() {
        assert_eq!(hex([0x00, 0x0f, 0xab, 0xff]), "000fabff");
        assert_eq!(hex([]), "");
    }
}
