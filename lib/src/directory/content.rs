//! Directory content

use super::entry_data::EntryData;
use crate::{
    crypto::sign::PublicKey,
    error::{Error, Result},
};
use std::collections::BTreeMap;

pub(super) type Content = BTreeMap<String, BTreeMap<PublicKey, EntryData>>;

pub(super) fn serialize(content: &Content) -> Vec<u8> {
    bincode::serialize(content).expect("failed to serialize directory content")
}

pub(super) fn deserialize(input: &[u8]) -> Result<Content> {
    bincode::deserialize(input).map_err(Error::MalformedDirectory)
}
