//! TOML decoding with go-toml v2 semantics.
//!
//! go-toml matches a key to a struct field by exact name first, then
//! case-insensitively, and ignores unknown keys. Serde matches exactly, so
//! [`from_str`] decodes through [`Fold`], which renames table keys to the
//! struct's field names before serde sees them.

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess,
    Visitor,
};
use toml::Value;

use crate::Error;

/// Parses `src` into `T` the way go-toml's `Unmarshal` would.
pub fn from_str<T: DeserializeOwned>(src: &str) -> Result<T, Error> {
    let table: toml::Table = toml::from_str(src).map_err(|e| Error(go_message(&e, src)))?;
    T::deserialize(Fold(Value::Table(table))).map_err(|e| Error(format!("toml: {}", e.message())))
}

/// One-line `toml: …` text for a syntax error (go-toml prints no snippet).
fn go_message(e: &toml::de::Error, src: &str) -> String {
    let msg = e.message().trim().replace('\n', " ");
    match e.span() {
        Some(span) => {
            let before = &src[..span.start.min(src.len())];
            let line = before.matches('\n').count() + 1;
            let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
            format!("toml: line {line}, column {col}: {msg}")
        }
        None => format!("toml: {msg}"),
    }
}

struct Fold(Value);

impl<'de> Deserializer<'de> for Fold {
    type Error = toml::de::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Table(t) => visitor.visit_map(FoldMap::new(t, None)),
            Value::Array(a) => visitor.visit_seq(FoldSeq(a.into_iter())),
            other => other.into_deserializer().deserialize_any(visitor),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Table(t) => visitor.visit_map(FoldMap::new(t, Some(fields))),
            other => other
                .into_deserializer()
                .deserialize_struct(name, fields, visitor),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0
            .into_deserializer()
            .deserialize_enum(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map identifier ignored_any
    }
}

struct FoldMap {
    entries: std::vec::IntoIter<(String, Value)>,
    value: Option<Value>,
}

impl FoldMap {
    fn new(table: toml::Table, fields: Option<&'static [&'static str]>) -> Self {
        let entries: Vec<(String, Value)> = table
            .into_iter()
            .map(|(k, v)| {
                let key = match fields {
                    Some(fields) if !fields.contains(&k.as_str()) => {
                        let lower = k.to_lowercase();
                        fields
                            .iter()
                            .find(|f| f.to_lowercase() == lower)
                            .map_or(k, |f| (*f).to_string())
                    }
                    _ => k,
                };
                (key, v)
            })
            .collect();
        FoldMap {
            entries: entries.into_iter(),
            value: None,
        }
    }
}

impl<'de> MapAccess<'de> for FoldMap {
    type Error = toml::de::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.entries.next() {
            Some((k, v)) => {
                self.value = Some(v);
                seed.deserialize(k.into_deserializer()).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        match self.value.take() {
            Some(v) => seed.deserialize(Fold(v)),
            None => Err(de::Error::custom("value is missing")),
        }
    }
}

struct FoldSeq(std::vec::IntoIter<Value>);

impl<'de> SeqAccess<'de> for FoldSeq {
    type Error = toml::de::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        match self.0.next() {
            Some(v) => seed.deserialize(Fold(v)).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Inner {
        #[serde(rename = "fileGlob")]
        file_glob: String,
        order: i64,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Outer {
        #[serde(rename = "command")]
        commands: Vec<Inner>,
        name: Option<String>,
    }

    #[test]
    fn keys_match_case_insensitively() {
        let got: Outer =
            from_str("Name = \"x\"\n[[Command]]\nFILEGLOB = \"*.go\"\nOrder = 3\nunknown = 1\n")
                .unwrap();
        assert_eq!(got.name.as_deref(), Some("x"));
        assert_eq!(got.commands.len(), 1);
        assert_eq!(got.commands[0].file_glob, "*.go");
        assert_eq!(got.commands[0].order, 3);
    }

    #[test]
    fn errors_are_single_line() {
        let err = from_str::<Outer>("name = \n").unwrap_err();
        assert!(err.0.starts_with("toml: line 1"), "{err}");
        assert!(!err.0.contains('\n'));
        let err = from_str::<Outer>("name = 3\n").unwrap_err();
        assert!(err.0.starts_with("toml: "), "{err}");
    }
}
