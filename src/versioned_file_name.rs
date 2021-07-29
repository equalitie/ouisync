use crate::replica_id::ReplicaId;
use std::fmt;

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct VersionedFileName<'a> {
    name: &'a str,
    branch_id: Option<ReplicaId>,
}

impl<'a> VersionedFileName<'a> {
    pub fn unique(name: &'a str) -> Self {
        Self {
            name,
            branch_id: None,
        }
    }

    pub fn concurrent(name: &'a str, branch_id: ReplicaId) -> Self {
        Self {
            name,
            branch_id: Some(branch_id),
        }
    }

    pub fn to_unversioned(&self) -> &'a str {
        self.name
    }
}

impl<'a> fmt::Display for VersionedFileName<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(branch_id) = &self.branch_id {
            write!(
                f,
                "{}{}{:3$x}",
                self.name, SUFFIX_SEPARATOR, branch_id, SUFFIX_LEN
            )
        } else {
            write!(f, "{}", self.name)
        }
    }
}

const SUFFIX_LEN: usize = 8;
const SUFFIX_SEPARATOR: &str = ".v";

pub fn partial_parse(name: &str) -> (&str, Option<[u8; SUFFIX_LEN * 2]>) {
    let index = if let Some(index) = name.len().checked_sub(SUFFIX_LEN + SUFFIX_SEPARATOR.len()) {
        index
    } else {
        return (name, None);
    };

    if &name[index..index + SUFFIX_SEPARATOR.len()] != SUFFIX_SEPARATOR {
        return (name, None);
    }

    let mut suffix = [0; SUFFIX_LEN * 2];

    if hex::decode_to_slice(&name[index + 2..], &mut suffix).is_ok() {
        (&name[..index], Some(suffix))
    } else {
        (name, None)
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::test_utils;
    // use test_strategy::proptest;

    // #[proptest]
    // fn partial_parse_of_valid_versioned_name(
    //     #[strategy(1..64)] name_len: usize,
    //     #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    // ) {
    //     let mut rng = StdRng::seed_from_u64(rng_seed);
    // }
}
