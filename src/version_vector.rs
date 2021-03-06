use crate::replica_id::ReplicaId;
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};
use std::{cmp::Ordering, collections::HashMap};

/// [Version vector](https://en.wikipedia.org/wiki/Version_vector).
///
/// The `PartialOrd` impl provides the "happened-before" relation like follows:
///
/// - `Some(Ordering::Equal)`   -> the vectors are exactly equal
/// - `Some(Ordering::Less)`    -> the lhs vector happened-before the rhs vector
/// - `Some(Ordering::Greater)` -> the rhs vector happened-before the lhs vector
/// - `None`                    -> the version vectors are concurrent
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct VersionVector(HashMap<ReplicaId, u64>);

impl VersionVector {
    /// Creates an empty version vector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts an entry into this version vector. If the entry already exists, it's overwritten
    /// only if the new version is higher than the existing version.
    pub fn insert(&mut self, replica_id: ReplicaId, version: u64) {
        let old = self.0.entry(replica_id).or_insert(0);
        *old = (*old).max(version);
    }

    /// Retrieves the version corresponding to the given replica id.
    pub fn get(&self, replica_id: &ReplicaId) -> u64 {
        self.0.get(replica_id).copied().unwrap_or(0)
    }

    /// Increments the version corresponding to the given replica id and returns it.
    pub fn increment(&mut self, replica_id: ReplicaId) -> u64 {
        let version = self.0.entry(replica_id).or_insert(0);
        *version += 1;
        *version
    }

    /// Merge two versio vectors into one. The version of each entry in the resulting vector is
    /// the maximum of the corresponding entries of the input vectors.
    pub fn merge(&mut self, other: Self) {
        for (replica_id, version) in other.0 {
            self.insert(replica_id, version)
        }
    }
}

impl PartialOrd for VersionVector {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use Ordering::*;

        self.0
            .iter()
            .map(|(lhs_key, &lhs_version)| (lhs_version, other.get(lhs_key)))
            .chain(
                other
                    .0
                    .iter()
                    .filter(|(rhs_key, _)| !self.0.contains_key(rhs_key))
                    .map(|(_, &rhs_version)| (0, rhs_version)),
            )
            .try_fold(Equal, |ordering, (lhs_version, rhs_version)| {
                match (ordering, lhs_version.cmp(&rhs_version)) {
                    (Equal, Equal) => Some(Equal),
                    (Equal, Less) | (Less, Equal) | (Less, Less) => Some(Less),
                    (Equal, Greater) | (Greater, Equal) | (Greater, Greater) => Some(Greater),
                    (Less, Greater) | (Greater, Less) => None,
                }
            })
    }
}

impl PartialEq for VersionVector {
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other) == Some(Ordering::Equal)
    }
}

impl Eq for VersionVector {}

// Support reading/writing `VersionVector` directly from/to the db:

impl Type<Sqlite> for VersionVector {
    fn type_info() -> SqliteTypeInfo {
        Vec::<u8>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for VersionVector {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        bincode::serialize(self)
            .expect("failed to serialize VersionVector for db")
            .encode_by_ref(args)
    }
}

impl<'r> Decode<'r, Sqlite> for VersionVector {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let slice = <&[u8]>::decode(value)?;
        Ok(bincode::deserialize(slice)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! vv {
        ($($key:expr => $version:expr),*) => {{
            #[allow(unused_mut)]
            let mut vv = VersionVector::new();
            $(
                vv.insert($key, $version);
            )*
            vv
        }};
    }

    #[test]
    fn eq() {
        let id0 = rand::random();
        let id1 = rand::random();

        assert_eq!(vv![], vv![]);
        assert_eq!(vv![id0 => 0], vv![id0 => 0]);
        assert_eq!(vv![id0 => 1], vv![id0 => 1]);
        assert_eq!(vv![id0 => 0, id1 => 1], vv![id0 => 0, id1 => 1]);
        assert_eq!(vv![id0 => 0, id1 => 1], vv![id1 => 1]);
    }

    #[test]
    fn cmp_equal() {
        let id0 = rand::random();
        let id1 = rand::random();

        assert_eq!(vv![].partial_cmp(&vv![]), Some(Ordering::Equal));
        assert_eq!(
            vv![id0 => 0].partial_cmp(&vv![id0 => 0]),
            Some(Ordering::Equal)
        );
        assert_eq!(
            vv![id0 => 1].partial_cmp(&vv![id0 => 1]),
            Some(Ordering::Equal)
        );
        assert_eq!(
            vv![id0 => 0, id1 => 1].partial_cmp(&vv![id0 => 0, id1 => 1]),
            Some(Ordering::Equal)
        );
        assert_eq!(
            vv![id0 => 0, id1 => 1].partial_cmp(&vv![id1 => 1]),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn cmp_less() {
        let id0 = rand::random();
        let id1 = rand::random();

        assert_eq!(vv![].partial_cmp(&vv![id0 => 1]), Some(Ordering::Less));

        assert_eq!(
            vv![id0 => 0].partial_cmp(&vv![id0 => 1]),
            Some(Ordering::Less)
        );

        assert_eq!(
            vv![id0 => 0].partial_cmp(&vv![id0 => 1, id1 => 1]),
            Some(Ordering::Less)
        );

        assert_eq!(
            vv![id0 => 0, id1 => 0].partial_cmp(&vv![id0 => 1, id1 => 1]),
            Some(Ordering::Less)
        );

        assert_eq!(
            vv![id0 => 0, id1 => 0].partial_cmp(&vv![id0 => 0, id1 => 1]),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn cmp_greater() {
        let id0 = rand::random();
        let id1 = rand::random();

        assert_eq!(vv![id0 => 1].partial_cmp(&vv![]), Some(Ordering::Greater));

        assert_eq!(
            vv![id0 => 1].partial_cmp(&vv![id0 => 0]),
            Some(Ordering::Greater)
        );

        assert_eq!(
            vv![
                id0 => 1, id1 => 1]
            .partial_cmp(&vv![id0 => 0]),
            Some(Ordering::Greater)
        );

        assert_eq!(
            vv![id0 => 1, id1 => 1].partial_cmp(&vv![id0 => 0, id1 => 0]),
            Some(Ordering::Greater)
        );

        assert_eq!(
            vv![id0 => 1, id1 => 1].partial_cmp(&vv![id0 => 1, id1 => 0]),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn cmp_concurrent() {
        let id0 = rand::random();
        let id1 = rand::random();

        assert_eq!(
            vv![id0 => 0, id1 => 1].partial_cmp(&vv![id0 => 1, id1 => 0]),
            None
        );

        assert_eq!(vv![id1 => 1].partial_cmp(&vv![id0 => 1]), None);
    }

    #[test]
    fn insert() {
        let id = rand::random();

        let mut vv = vv![];
        assert_eq!(vv.get(&id), 0);

        vv.insert(id, 1);
        assert_eq!(vv.get(&id), 1);

        vv.insert(id, 2);
        assert_eq!(vv.get(&id), 2);

        vv.insert(id, 1);
        assert_eq!(vv.get(&id), 2);
    }

    #[test]
    fn increment() {
        let id = rand::random();

        let mut vv = vv![];
        assert_eq!(vv.get(&id), 0);

        assert_eq!(vv.increment(id), 1);
        assert_eq!(vv.get(&id), 1);
    }

    #[test]
    fn merge() {
        let id0 = rand::random();
        let id1 = rand::random();

        let mut vv = vv![];
        vv.merge(vv![]);
        assert_eq!(vv, vv![]);

        let mut vv = vv![];
        vv.merge(vv![id0 => 1]);
        assert_eq!(vv, vv![id0 => 1]);

        let mut vv = vv![id0 => 1];
        vv.merge(vv![]);
        assert_eq!(vv, vv![id0 => 1]);

        let mut vv = vv![id0 => 1];
        vv.merge(vv![id0 => 2]);
        assert_eq!(vv, vv![id0 => 2]);

        let mut vv = vv![id0 => 2];
        vv.merge(vv![id0 => 1]);
        assert_eq!(vv, vv![id0 => 2]);

        let mut vv = vv![id0 => 1];
        vv.merge(vv![id1 => 2]);
        assert_eq!(vv, vv![id0 => 1, id1 => 2]);

        let mut vv = vv![id0 => 1, id1 => 2];
        vv.merge(vv![id0 => 2, id1 => 1]);
        assert_eq!(vv, vv![id0 => 2, id1 => 2]);
    }
}
