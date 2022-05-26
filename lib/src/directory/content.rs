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

type ContentV0 = BTreeMap<String, BTreeMap<PublicKey, EntryData>>;
type ContentV1 = BTreeMap<String, EntryData>;

pub(super) fn serialize(content: &Content) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(vint64::encode(VERSION).as_ref());
    bincode::serialize_into(&mut output, content).expect("failed to serialize directory content");
    output
}

pub(super) fn deserialize(mut input: &[u8]) -> Result<Content> {
    let version = vint64::decode(&mut input).map_err(|_| Error::MalformedDirectory)?;

    if version == VERSION {
        bincode::deserialize(input).map_err(|_| Error::MalformedDirectory)
    } else {
        Err(Error::StorageVersionMismatch)
    }
}
