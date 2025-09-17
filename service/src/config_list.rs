use std::{borrow::Borrow, fmt};

use serde::{Deserialize, Deserializer};

// Wrapper for `Vec<T>` which deserializes from either a list of `T`s or from a single `T`.
#[derive(Eq, PartialEq)]
pub(crate) struct ConfigList<T>(Vec<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ConfigList<T> {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany<T> {
            Many(Vec<T>),
            One(T),
        }

        match OneOrMany::deserialize(de)? {
            OneOrMany::Many(items) => Ok(Self(items)),
            OneOrMany::One(item) => Ok(Self(vec![item])),
        }
    }
}

impl<T> fmt::Debug for ConfigList<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> From<ConfigList<T>> for Vec<T> {
    fn from(value: ConfigList<T>) -> Self {
        value.0
    }
}

impl<T> Borrow<Vec<T>> for ConfigList<T> {
    fn borrow(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> Borrow<[T]> for ConfigList<T> {
    fn borrow(&self) -> &[T] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize() {
        assert_eq!(
            ConfigList(Vec::new()),
            serde_json::from_str::<ConfigList<&str>>("[]").unwrap()
        );
        assert_eq!(
            ConfigList(vec!["foo"]),
            serde_json::from_str::<ConfigList<&str>>("[\"foo\"]").unwrap()
        );
        assert_eq!(
            ConfigList(vec!["foo", "bar"]),
            serde_json::from_str::<ConfigList<&str>>("[\"foo\", \"bar\"]").unwrap()
        );
        assert_eq!(
            ConfigList(vec!["foo"]),
            serde_json::from_str::<ConfigList<&str>>("\"foo\"").unwrap()
        );
    }

    #[test]
    fn roundtrip() {
        let values = vec!["foo", "bar"];
        let serialized = serde_json::to_string(&values).unwrap();
        let deserialized: ConfigList<&str> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, ConfigList(values));

        let value = "foo";
        let serialized = serde_json::to_string(&value).unwrap();
        let deserialized: ConfigList<&str> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, ConfigList(vec![value]));
    }
}
