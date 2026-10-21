/// Declares a Go string-typed enum (`type X string` plus consts).
///
/// Known values get variants; anything else (including the empty Go zero
/// value, which is `Default`) round-trips through `Other`, so an unknown
/// persisted value never fails to load. Equality and hashing go through
/// `as_str`, so `Other("merge")` equals `Merge`.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// Any other stored value, including the empty Go zero value.
            Other(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $value, )+
                    Self::Other(s) => s,
                }
            }

            /// Reports whether the value is one of the declared constants.
            pub fn is_known(&self) -> bool {
                $( self.as_str() == $value || )+ false
            }

            /// Reports whether this is the empty Go zero value.
            pub fn is_empty(&self) -> bool {
                self.as_str().is_empty()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::Other(String::new())
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                match s {
                    $( $value => Self::$variant, )+
                    _ => Self::Other(s.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                match s.as_str() {
                    $( $value => Self::$variant, )+
                    _ => Self::Other(s),
                }
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.as_str() == other.as_str()
            }
        }

        impl Eq for $name {}

        impl std::hash::Hash for $name {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.as_str().hash(state);
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Ok(String::deserialize(d)?.into())
            }
        }
    };
}
