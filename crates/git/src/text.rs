//! String semantics the crate relies on.

/// Equal under simple Unicode case folding.
pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    let mut x = a.chars();
    let mut y = b.chars();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(c), Some(d)) => {
                if c != d && !fold_eq(c, d) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn fold_eq(c: char, d: char) -> bool {
    let lower = |ch: char| ch.to_lowercase().next().unwrap_or(ch);
    let upper = |ch: char| ch.to_uppercase().next().unwrap_or(ch);
    lower(c) == lower(d) || upper(c) == upper(d)
}

pub(crate) fn atoi(s: &str) -> Option<i64> {
    s.parse().ok()
}
