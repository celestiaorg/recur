//! RFC 9562 UUID v7 ids for queued sends and held prompts.

pub fn uuidv7() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuidv7_is_rfc_9562_version_7() {
        let id = uuidv7();
        let b = id.as_bytes();
        assert_eq!(id.len(), 36);
        for (i, c) in id.chars().enumerate() {
            match i {
                8 | 13 | 18 | 23 => assert_eq!(c, '-'),
                _ => assert!(c.is_ascii_digit() || ('a'..='f').contains(&c), "{id}"),
            }
        }
        assert_eq!(b[14], b'7');
        assert!(matches!(b[19], b'8' | b'9' | b'a' | b'b'), "{id}");
    }
}
