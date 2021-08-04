use crate::replica_id::ReplicaId;

const SUFFIX_LEN: usize = 8;
const SUFFIX_SEPARATOR: &str = ".v";

pub fn create(name: &str, branch_id: &ReplicaId) -> String {
    format!("{}{}{:-3$x}", name, SUFFIX_SEPARATOR, branch_id, SUFFIX_LEN)
}

pub fn parse(name: &str) -> (&str, Option<[u8; SUFFIX_LEN / 2]>) {
    let index = if let Some(index) = name.len().checked_sub(SUFFIX_LEN + SUFFIX_SEPARATOR.len()) {
        index
    } else {
        return (name, None);
    };

    if &name[index..index + SUFFIX_SEPARATOR.len()] != SUFFIX_SEPARATOR {
        return (name, None);
    }

    let mut suffix = [0; SUFFIX_LEN / 2];

    if hex::decode_to_slice(&name[index + 2..], &mut suffix).is_ok() {
        (&name[..index], Some(suffix))
    } else {
        (name, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use std::convert::TryFrom;
    use test_strategy::proptest;

    #[test]
    fn create_versioned_file_name() {
        let replica_id = [
            0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let replica_id = ReplicaId::try_from(&replica_id[..]).unwrap();

        assert_eq!(create("file.txt", &replica_id), "file.txt.vdeadbeef");
    }

    #[proptest]
    fn parse_versioned_file_name(
        base_name: String,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let branch_id: ReplicaId = rng.gen();
        let versioned_name = create(&base_name, &branch_id);

        let (parsed_base_name, branch_id_prefix) = parse(&versioned_name);
        let branch_id_prefix = branch_id_prefix.unwrap();

        assert_eq!(parsed_base_name, base_name);
        assert!(branch_id.starts_with(&branch_id_prefix));
    }
}
