//! The preferred Open In target (`lib/open-in-pref.ts`), kept in the app's local storage.

use crate::LocalStorage;

const KEY: &str = "recur.openInDefault";

/// The saved default target id; empty keeps catalog order.
pub fn open_in_default(storage: &LocalStorage) -> String {
    storage.get(KEY).unwrap_or("").to_string()
}

/// `setOpenInDefault`: `copy-path` is never a default; empty clears it.
pub fn set_open_in_default(storage: &mut LocalStorage, id: &str) {
    let next = id.trim();
    if next == "copy-path" {
        return;
    }
    if next.is_empty() {
        storage.remove(KEY);
    } else {
        storage.set(KEY, next);
    }
}

/// `orderOpenInTargets`: the default first; unchanged when it is unknown or already first.
pub fn order_open_in_targets<T: Clone>(
    list: &[T],
    default_id: &str,
    id: impl Fn(&T) -> &str,
) -> Vec<T> {
    if default_id.is_empty() {
        return list.to_vec();
    }
    match list.iter().position(|t| id(t) == default_id) {
        None | Some(0) => list.to_vec(),
        Some(i) => {
            let mut out = vec![list[i].clone()];
            out.extend(list[..i].iter().cloned());
            out.extend(list[i + 1..].iter().cloned());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&'static str], default_id: &str) -> Vec<&'static str> {
        order_open_in_targets(list, default_id, |s| s)
    }

    const LIST: [&str; 3] = ["finder", "cursor", "terminal"];

    #[test]
    fn empty_default_keeps_catalog_order() {
        assert_eq!(ids(&LIST, ""), LIST);
    }

    #[test]
    fn unknown_default_keeps_catalog_order() {
        assert_eq!(ids(&LIST, "zed"), LIST);
    }

    #[test]
    fn moves_the_default_to_the_front() {
        assert_eq!(ids(&LIST, "cursor"), ["cursor", "finder", "terminal"]);
        assert_eq!(ids(&LIST, "terminal"), ["terminal", "finder", "cursor"]);
    }

    #[test]
    fn already_first_default_is_unchanged() {
        assert_eq!(ids(&LIST, "finder"), LIST);
    }

    #[test]
    fn persists_and_clears() {
        let mut s = LocalStorage::in_memory();
        set_open_in_default(&mut s, " cursor ");
        assert_eq!(open_in_default(&s), "cursor");
        set_open_in_default(&mut s, "copy-path");
        assert_eq!(open_in_default(&s), "cursor");
        set_open_in_default(&mut s, "");
        assert_eq!(open_in_default(&s), "");
    }
}
