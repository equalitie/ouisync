//! Directory content

use super::entry_data::EntryData;
use crate::{
    crypto::sign::PublicKey,
    error::{Error, Result},
};
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::{btree_map, BTreeMap};

/// Version of the Directory serialization format.
pub(crate) const VERSION: u64 = 1;

pub(super) type Content = ContentV1;

type ContentV1 = BTreeMap<String, EntryData>;
type ContentV0 = BTreeMap<String, BTreeMap<PublicKey, EntryData>>;

pub(super) fn serialize(old_content: &Content, new_entry: Option<(&str, &EntryData)>) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(vint64::encode(VERSION).as_ref());
    bincode::serialize_into(
        &mut output,
        &UpdatedContent {
            old: old_content,
            new: new_entry,
        },
    )
    .expect("failed to serialize directory content");
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

struct UpdatedContent<'a> {
    old: &'a Content,
    new: Option<(&'a str, &'a EntryData)>,
}

impl<'a> UpdatedContent<'a> {
    fn iter(&self) -> Iter {
        Iter {
            old: self.old.iter(),
            new: self.new,
        }
    }

    fn len(&self) -> usize {
        if let Some((name, _)) = self.new {
            if self.old.contains_key(name) {
                self.old.len()
            } else {
                self.old.len() + 1
            }
        } else {
            self.old.len()
        }
    }
}

impl<'a> Serialize for UpdatedContent<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

struct Iter<'a> {
    old: btree_map::Iter<'a, String, EntryData>,
    new: Option<(&'a str, &'a EntryData)>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a str, &'a EntryData);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((old_name, old_data)) = self.old.next() {
            if let Some((new_name, _)) = &self.new {
                if old_name == new_name {
                    self.new.take()
                } else {
                    Some((old_name, old_data))
                }
            } else {
                Some((old_name, old_data))
            }
        } else {
            self.new.take()
        }
    }
}
