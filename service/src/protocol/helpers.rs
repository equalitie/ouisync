use std::{fmt, ops::Deref};

use serde::{Deserialize, Serialize};

/// Simple wrapper for `Vec<u8>` with a custom `Debug` impl that doesn't print the whole content to
/// prevent spamming logs.
#[derive(Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes(#[serde(with = "serde_bytes")] Vec<u8>);

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(b: Bytes) -> Self {
        b.0
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{} bytes]", self.0.len())
    }
}

/// Serialize / deserialize type by converting it to/from string.
pub(super) mod str {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{fmt, str::FromStr};

    pub fn deserialize<'de, D, T>(d: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: fmt::Display,
    {
        let s = <&str>::deserialize(d)?;
        let v = s.parse().map_err(serde::de::Error::custom)?;
        Ok(v)
    }

    pub fn serialize<T, S>(value: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: fmt::Display,
        S: Serializer,
    {
        value.to_string().serialize(s)
    }
}

/// Serialize / deserialize vec by converting its elements to/from strings.
pub(super) mod strs {
    use serde::{de, ser::SerializeSeq, Deserializer, Serializer};
    use std::{fmt, marker::PhantomData, str::FromStr};

    pub fn deserialize<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: fmt::Display,
    {
        struct Visitor<T>(PhantomData<T>);

        impl<'de, T> de::Visitor<'de> for Visitor<T>
        where
            T: FromStr,
            T::Err: fmt::Display,
        {
            type Value = Vec<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "sequence of strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));

                while let Some(item) = seq.next_element::<&str>()? {
                    out.push(item.parse().map_err(<A::Error as de::Error>::custom)?);
                }

                Ok(out)
            }
        }

        d.deserialize_seq(Visitor(PhantomData))
    }

    pub fn serialize<T, S>(value: &[T], s: S) -> Result<S::Ok, S::Error>
    where
        T: fmt::Display,
        S: Serializer,
    {
        use std::fmt::Write;

        let mut buffer = String::new();
        let mut s = s.serialize_seq(Some(value.len()))?;

        for item in value {
            write!(&mut buffer, "{}", item).expect("failed to format item");
            s.serialize_element(&buffer)?;
            buffer.clear();
        }
        s.end()
    }
}

/// Serialize / deserialize Duration by converting it to/from milliseconds
pub(super) mod millis {
    use std::time::Duration;

    use serde::{ser, Deserialize, Deserializer, Serializer};

    pub fn deserialize<'de, D>(d: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(d)?;
        Ok(Duration::from_millis(millis))
    }

    pub fn serialize<S>(value: &Duration, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_u64(value.as_millis().try_into().map_err(ser::Error::custom)?)
    }
}
