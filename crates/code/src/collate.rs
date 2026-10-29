//! The UI's `localeCompare` / `Intl.Collator()` order: ICU root collation,
//! tertiary strength, punctuation not ignored.

use std::cmp::Ordering;
use std::sync::LazyLock;

use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed};

static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
    Collator::try_new(Default::default(), CollatorOptions::default())
        .expect("compiled ICU root collation data")
});

/// JavaScript `a.localeCompare(b)` in the default (root) locale.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    COLLATOR.compare(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_collation_like_the_webview() {
        // Underscore sorts before dot, and letters after punctuation.
        assert_eq!(locale_compare("seed_prs.go", "seed.go"), Ordering::Less);
        assert_eq!(locale_compare("a/b", "a_b"), Ordering::Greater);
        assert_eq!(locale_compare("a", "B"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
        assert_eq!(locale_compare("x", "x"), Ordering::Equal);
    }
}
