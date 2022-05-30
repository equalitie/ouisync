//! Directory content

use super::entry_data::EntryData;
use crate::{
    crypto::sign::PublicKey,
    error::{Error, Result},
};
use std::collections::BTreeMap;

/// Version of the Directory serialization format.
pub(crate) const VERSION: u64 = 1;

pub(super) type Content = ContentV1;

type ContentV1 = BTreeMap<String, EntryData>;
type ContentV0 = BTreeMap<String, BTreeMap<PublicKey, EntryData>>;

pub(super) fn serialize(content: &Content) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(vint64::encode(VERSION).as_ref());
    bincode::serialize_into(&mut output, content).expect("failed to serialize directory content");
    output
}

pub(super) fn deserialize(mut input: &[u8]) -> Result<Content> {
    let version = vint64::decode(&mut input).map_err(|_| Error::MalformedDirectory)?;

    match version {
        VERSION => bincode::deserialize(input).map_err(|_| Error::MalformedDirectory),
        0 => Ok(upgrade_from_v0(
            bincode::deserialize(input).map_err(|_| Error::MalformedDirectory)?,
        )),
        _ => Err(Error::StorageVersionMismatch),
    }
}

fn upgrade_from_v0(v0: ContentV0) -> ContentV1 {
    use crate::versioned_file_name;

    let mut v1 = BTreeMap::new();

    for (name, versions) in v0 {
        if versions.len() <= 1 {
            // If there is only one version, insert it directly
            if let Some(data) = versions.into_values().next() {
                v1.insert(name, data);
            }
        } else {
            // If there is more than one version, create unique name for each of them and insert
            // them as separate entries
            for (author_id, data) in versions {
                v1.insert(versioned_file_name::create(&name, &author_id), data);
            }
        }
    }

    v1
}
