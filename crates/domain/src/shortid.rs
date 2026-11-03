//! Compact, unambiguous public identifiers.
//!
//! IDs look like "find_k7mq2xf9": a lowercase prefix, an underscore, and a
//! random suffix drawn from a 30-character alphabet that omits i/l/o/0/1 so
//! humans (and models) are less likely to misread them.

/// 30 chars, no i/l/o/0/1.
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstvwxyz23456789";
/// The suffix always starts with a letter.
const LETTERS: &[u8] = b"abcdefghjkmnpqrstvwxyz";

pub const MIN_LENGTH: i64 = 6;
pub const MAX_LENGTH: i64 = 10;

/// The collision-retry budget for [`new_unique`] and insert loops.
pub const DEFAULT_ATTEMPTS: i64 = 8;

/// The well-formed id pattern (prefix + MIN_LENGTH–MAX_LENGTH suffix). The
/// first suffix character is always a letter so ids never look like find_2….
pub const VALID_PATTERN: &str =
    "^[a-z]+_[abcdefghjkmnpqrstvwxyz][abcdefghjkmnpqrstvwxyz23456789]{5,9}$";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("shortid: prefix {0:?} must be lowercase letters")]
    Prefix(String),
    #[error("shortid: length must be {MIN_LENGTH}-{MAX_LENGTH}")]
    Length,
    #[error("shortid: {0}")]
    Random(getrandom::Error),
    /// No unused id within the attempt budget.
    #[error("shortid: exhausted {0} attempts: shortid: collision")]
    Collision(i64),
}

/// Generates an ID like "find_k7mq2xf9".
pub fn new(prefix: &str, length: i64) -> Result<String, Error> {
    if !valid_prefix(prefix) {
        return Err(Error::Prefix(prefix.to_owned()));
    }
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&length) {
        return Err(Error::Length);
    }
    let suffix = random_string(length as usize).map_err(Error::Random)?;
    Ok(format!("{prefix}_{suffix}"))
}

/// Generates IDs until `taken` reports the candidate is free, or `attempts`
/// is exhausted. A `None` taken treats every candidate as free.
/// `attempts <= 0` uses [`DEFAULT_ATTEMPTS`].
pub fn new_unique(
    prefix: &str,
    length: i64,
    attempts: i64,
    mut taken: Option<&mut dyn FnMut(&str) -> bool>,
) -> Result<String, Error> {
    let attempts = if attempts <= 0 {
        DEFAULT_ATTEMPTS
    } else {
        attempts
    };
    for _ in 0..attempts {
        let id = new(prefix, length)?;
        if !taken.as_mut().is_some_and(|taken| taken(&id)) {
            return Ok(id);
        }
    }
    Err(Error::Collision(attempts))
}

/// Reports whether id matches [`VALID_PATTERN`].
pub fn is_valid(id: &str) -> bool {
    let Some((prefix, suffix)) = id.split_once('_') else {
        return false;
    };
    let suffix = suffix.as_bytes();
    valid_prefix(prefix)
        && (MIN_LENGTH as usize..=MAX_LENGTH as usize).contains(&suffix.len())
        && LETTERS.contains(&suffix[0])
        && suffix.iter().all(|b| ALPHABET.contains(b))
}

fn valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_lowercase())
}

fn random_string(length: usize) -> Result<String, getrandom::Error> {
    let mut out = String::with_capacity(length);
    let letter_max = reject_max(LETTERS.len());
    let alpha_max = reject_max(ALPHABET.len());
    let mut buf = vec![0u8; length * 2]; // over-fetch to reduce loop iterations
    while out.len() < length {
        getrandom::fill(&mut buf)?;
        for &b in &buf {
            let (alpha, max) = if out.is_empty() {
                (LETTERS, letter_max)
            } else {
                (ALPHABET, alpha_max)
            };
            if usize::from(b) < max {
                out.push(char::from(alpha[usize::from(b) % alpha.len()]));
                if out.len() == length {
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// The exclusive upper bound for unbiased rejection sampling over n symbols.
/// When n divides 256 every byte maps uniformly, so the bound is 256.
fn reject_max(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    match 256 % n {
        0 => 256,
        rem => 256 - rem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn suffix_start(id: &str) -> u8 {
        id.split_once('_')
            .and_then(|(_, s)| s.bytes().next())
            .unwrap_or(0)
    }

    #[test]
    fn new_generates_valid_id() {
        let id = new("find", 8).unwrap();
        assert!(is_valid(&id), "{id}");
        assert_eq!(&id[..5], "find_");
        assert_eq!(id.len(), "find_".len() + 8);
        assert!(suffix_start(&id).is_ascii_lowercase(), "{id}");
    }

    #[test]
    fn new_rejects_bad_input() {
        for (name, prefix, length) in [
            ("empty prefix", "", 8),
            ("uppercase prefix", "Find", 8),
            ("digit prefix", "f1", 8),
            ("zero length", "find", 0),
            ("negative length", "find", -1),
            ("below min", "find", MIN_LENGTH - 1),
            ("above max", "find", MAX_LENGTH + 1),
        ] {
            let err = new(prefix, length).expect_err(name);
            assert!(err.to_string().starts_with("shortid:"), "{name}: {err}");
        }
    }

    #[test]
    fn is_valid_cases() {
        for (id, want) in [
            ("find_k7mq2xf9", true),
            ("find_abcdef", true),
            ("usr_a34567", true),
            ("find_abcdefghjk", true), // 10 suffix
            ("usr_234567", false),     // suffix must start with a letter
            ("find_2abcdef", false),
            ("find_abcde", false), // 5 suffix
            ("find_abcdefghjkm", false),
            ("find_abcdei", false), // i not in alphabet
            ("find_abcde0", false),
            ("FIND_k7mq2xf9", false),
            ("find-k7mq2xf9", false),
            ("k7mq2xf9", false),
            ("", false),
        ] {
            assert_eq!(is_valid(id), want, "{id}");
        }
    }

    #[test]
    fn new_unique_retries_when_taken() {
        let mut seen = HashSet::new();
        let mut reject_first = true;
        let mut taken = |s: &str| {
            if reject_first {
                reject_first = false;
                seen.insert(s.to_owned());
                return true;
            }
            seen.contains(s)
        };
        let id = new_unique("find", 8, 8, Some(&mut taken)).unwrap();
        assert!(!seen.contains(&id), "{id}");
        assert!(is_valid(&id), "{id}");
    }

    #[test]
    fn new_unique_exhausted() {
        let err = new_unique("find", 8, 2, Some(&mut |_: &str| true)).unwrap_err();
        assert!(matches!(err, Error::Collision(2)), "{err}");
        assert!(err.to_string().ends_with("shortid: collision"));
    }

    #[test]
    fn new_unique_without_taken() {
        assert!(is_valid(&new_unique("find", 8, 0, None).unwrap()));
    }

    #[test]
    fn new_distinct() {
        let mut seen = HashSet::new();
        for _ in 0..64 {
            let id = new("find", 8).unwrap();
            assert!(seen.insert(id.clone()), "{id} already seen");
            assert!(suffix_start(&id).is_ascii_lowercase(), "{id}");
        }
    }

    #[test]
    fn reject_max_cases() {
        for (n, want) in [(30, 240), (22, 242), (32, 256), (1, 256), (0, 0)] {
            assert_eq!(reject_max(n), want, "{n}");
        }
    }
}
