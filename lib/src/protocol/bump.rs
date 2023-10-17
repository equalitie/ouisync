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

    pub fn apply(&self, lhs: &mut VersionVector) {
        match self {
            Self::Add(rhs) => *lhs += rhs,
            Self::Merge(rhs) => lhs.merge(rhs),
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
