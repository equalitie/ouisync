use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};
use std::{borrow::Borrow, ops::Deref, sync::Arc};

/// Identifier of a repository unique only within a single replica. To obtain a globally unique
/// identifier, it needs to be paired with a `ReplicaId`.
// TODO: remove the `Default` impl, instead provide a test-only `dummy` constructor.
#[derive(Default, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, sqlx::Type)]
#[serde(transparent)]
#[sqlx(transparent)]
pub(crate) struct RepositoryId(pub(super) u32);

/// Friendly, human-readable name of a repository.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Debug)]
pub(crate) struct RepositoryName(Arc<str>);

impl AsRef<str> for RepositoryName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for RepositoryName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Deref for RepositoryName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&'_ str> for RepositoryName {
    fn from(s: &str) -> Self {
        let s: Box<str> = s.into();
        Self(s.into())
    }
}

impl From<String> for RepositoryName {
    fn from(s: String) -> Self {
        Self(s.into_boxed_str().into())
    }
}

impl Type<Sqlite> for RepositoryName {
    fn type_info() -> SqliteTypeInfo {
        <&str>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for &'q RepositoryName {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        (*self).as_ref().encode_by_ref(args)
    }
}

impl<'r> Decode<'r, Sqlite> for RepositoryName {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(<&str>::decode(value)?.into())
    }
}
