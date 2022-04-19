//! Utilities for working with versioned entries.

use crate::{
    crypto::sign::PublicKey,
    directory::{EntryRef, FileRef},
    version_vector::VersionVector,
};
use std::cmp::Ordering;

pub(super) trait Versioned {
    fn version_vector(&self) -> &VersionVector;
    fn branch_id(&self) -> &PublicKey;
}

impl Versioned for EntryRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        EntryRef::version_vector(self)
    }

    fn branch_id(&self) -> &PublicKey {
        EntryRef::branch_id(self)
    }
}

impl Versioned for FileRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        FileRef::version_vector(self)
    }

    fn branch_id(&self) -> &PublicKey {
        FileRef::branch_id(self)
    }
}

// Returns the entries with the maximal version vectors.
pub(super) fn keep_maximal<E: Versioned>(
    entries: impl Iterator<Item = E>,
    local_branch_id: Option<&PublicKey>,
) -> Vec<E> {
    let mut max: Vec<E> = Vec::new();

    for new in entries {
        let mut insert = true;
        let mut remove = None;

        let new_is_local = local_branch_id == Some(new.branch_id());

        for (index, old) in max.iter().enumerate() {
            match (
                old.version_vector().partial_cmp(new.version_vector()),
                new_is_local,
            ) {
                // If both have identical versions, prefer the local one
                (Some(Ordering::Less), _) | (Some(Ordering::Equal), true) => {
                    insert = true;
                    remove = Some(index);
                    break;
                }
                (Some(Ordering::Greater), _) | (Some(Ordering::Equal), false) => {
                    insert = false;
                    break;
                }
                (None, _) => {
                    insert = true;
                }
            }
        }

        // Note: using `Vec::remove` to maintain the original order. Is there a more efficient
        // way?
        if let Some(index) = remove {
            max.remove(index);
        }

        if insert {
            max.push(new)
        }
    }

    max
}
