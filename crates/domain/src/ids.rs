//! Public short ids for persisted entities (`find_k7mq2xf9`).

use crate::shortid;

string_enum! {
    /// A registered short-id prefix. Add a variant when a new persisted
    /// entity gets a public id.
    IdType {
        Finding = "find",
    }
}

/// The suffix length for [`new_id`] (fits [`shortid::VALID_PATTERN`]).
pub const SHORT_ID_LENGTH: i64 = 8;

/// Generates a public short id for t, e.g. "find_k7mq2xf9".
pub fn new_id(t: &IdType) -> Result<String, shortid::Error> {
    shortid::new(t.as_str(), SHORT_ID_LENGTH)
}

/// Generates a public short id that `taken` reports as free.
pub fn new_unique_id(
    t: &IdType,
    taken: Option<&mut dyn FnMut(&str) -> bool>,
) -> Result<String, shortid::Error> {
    shortid::new_unique(
        t.as_str(),
        SHORT_ID_LENGTH,
        shortid::DEFAULT_ATTEMPTS,
        taken,
    )
}

/// Reports whether id is a well-formed public id for t.
pub fn valid_id(id: &str, t: &IdType) -> bool {
    matches!(id.split_once('_'), Some((prefix, _)) if prefix == t.as_str()) && shortid::is_valid(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Finding;

    #[test]
    fn new_id_finding() {
        let id = new_id(&IdType::Finding).unwrap();
        assert!(valid_id(&id, &IdType::Finding), "{id}");
        assert!(id.starts_with("find_"), "{id}");
    }

    #[test]
    fn new_unique_id_finding() {
        let id = new_unique_id(&IdType::Finding, Some(&mut |_: &str| false)).unwrap();
        assert!(valid_id(&id, &IdType::Finding), "{id}");
    }

    #[test]
    fn valid_id_cases() {
        assert!(valid_id("find_k7mq2xf9", &IdType::Finding));
        assert!(!valid_id("usr_k7mq2xf9", &IdType::Finding));
        assert!(!valid_id("find_abcde", &IdType::Finding));
    }

    #[test]
    fn finding_public_id_and_match() {
        let uid = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        let f = Finding {
            id: uid.into(),
            short_id: "find_k7mq2xf9".into(),
            ..Default::default()
        };
        assert_eq!(f.public_id(), "find_k7mq2xf9");
        assert!(f.matches_ref(uid) && f.matches_ref("find_k7mq2xf9"));
        assert!(!f.matches_ref("find_otherxxx") && !f.matches_ref(""));

        let legacy = Finding {
            id: "F-old-1".into(),
            ..Default::default()
        };
        assert_eq!(legacy.public_id(), "F-old-1");
        assert!(shortid::is_valid("find_k7mq2xf9"));
    }
}
