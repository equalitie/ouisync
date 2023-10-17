use crate::{crypto::sign::PublicKey, version_vector::VersionVector};

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
}

/// No-op Bump.
impl Default for Bump {
    fn default() -> Self {
        Self::Add(VersionVector::new())
    }
}
