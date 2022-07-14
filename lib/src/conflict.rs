use crate::crypto::sign::PublicKey;

const SUFFIX_LEN: usize = 8;
const SUFFIX_SEPARATOR: &str = ".v";

/// Create non-ambiguous name for a file/directory with `name` by appending a suffix derived from
/// `branch_id`.
pub fn create_unique_name(name: &str, branch_id: &PublicKey) -> String {
    format!("{}{}{:-3$x}", name, SUFFIX_SEPARATOR, branch_id, SUFFIX_LEN)
}

/// Parse a name created with `create_unique_name` into the original name and the disambiguation
/// suffix.
pub fn parse_unique_name(name: &str) -> (&str, Option<[u8; SUFFIX_LEN / 2]>) {
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
    use rand::{rngs::StdRng, SeedableRng};
    use test_strategy::proptest;

    #[test]
    fn create_disambiguated_file_name() {
        let writer_id = [
            0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let writer_id = PublicKey::try_from(&writer_id[..]).unwrap();

        assert_eq!(
            create_unique_name("file.txt", &writer_id),
            "file.txt.vdeadbeef"
        );
    }

    #[proptest]
    fn parse_disambiguated_file_name(
        base_name: String,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let branch_id = PublicKey::generate(&mut rng);
        let unique_name = create_unique_name(&base_name, &branch_id);

        let (parsed_base_name, branch_id_prefix) = parse_unique_name(&unique_name);
        let branch_id_prefix = branch_id_prefix.unwrap();

        assert_eq!(parsed_base_name, base_name);
        assert!(branch_id.starts_with(&branch_id_prefix));
    }
}
