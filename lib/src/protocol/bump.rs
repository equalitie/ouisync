use crate::{crypto::sign::PublicKey, version_vector::VersionVector};
use std::cmp::Ordering;

/// Operation on version vectors
///
/// Note: `Bump::default()` constructs a `Bump` whose `apply` is a no-op.
#[derive(Debug)]
pub(crate) enum Bump {
    Add(VersionVector),
    Merge(VersionVector),
}

impl Bump {
    /// Shortcut for `Self::Add(VersionVector::first(branch_id))`, for convenience.
    pub fn increment(branch_id: PublicKey) -> Self {
        Self::Add(VersionVector::first(branch_id))
    }

    /// Applies this bump to the `lhs` version vector. Returns the difference between the new and
    /// the old version vector.
    pub fn apply(self, lhs: &mut VersionVector) -> VersionVector {
        match self {
            Self::Add(rhs) => {
                *lhs += &rhs;
                rhs
            }
            Self::Merge(rhs) => {
                let diff = rhs.saturating_sub(lhs);
                lhs.merge(&rhs);
                diff
            }
        }
    }

    /// Checkes whether this bump would change the given version vector if applied to it.
    pub fn changes(&self, lhs: &VersionVector) -> bool {
        match self {
            Self::Add(rhs) => !rhs.is_empty(),
            Self::Merge(rhs) => matches!(lhs.partial_cmp(rhs), Some(Ordering::Less) | None),
        }
    }
}

/// No-op Bump.
impl Default for Bump {
    fn default() -> Self {
        Self::Add(VersionVector::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{arbitrary::any, collection::vec, sample::subsequence, strategy::Strategy};
    use test_strategy::proptest;

    #[proptest]
    fn apply_merge(
        #[strategy(version_vector_pair_strategy())] vvs: (VersionVector, VersionVector),
    ) {
        let (old, operand) = vvs;

        let new0 = old.clone().merged(&operand);
        let diff0 = new0.saturating_sub(&old);

        let mut new1 = old;
        let diff1 = Bump::Merge(operand).apply(&mut new1);

        assert_eq!(new0, new1);
        assert_eq!(diff0, diff1);
    }

    // Generates pair of version vector whose writer ids are taken from the same pool. This is
    // different from simply generating two random version vectors as they would be unlikely to
    // have any writer_ids in common.
    fn version_vector_pair_strategy() -> impl Strategy<Value = (VersionVector, VersionVector)> {
        let max_writers = 32;
        let max_version = 100u64;

        let writer_ids = vec(any::<PublicKey>(), 0..max_writers);

        writer_ids.prop_flat_map(move |writer_ids| {
            let a = version_vector_strategy(writer_ids.clone(), max_version);
            let b = version_vector_strategy(writer_ids, max_version);

            (a, b)
        })
    }

    fn version_vector_strategy(
        id_pool: Vec<PublicKey>,
        max_version: u64,
    ) -> impl Strategy<Value = VersionVector> {
        (0..=id_pool.len()).prop_flat_map(move |len| {
            let keys = subsequence(id_pool.clone(), len);
            let values = vec(0..max_version, len);

            (keys, values).prop_map(|(keys, values)| keys.into_iter().zip(values).collect())
        })
    }
}
