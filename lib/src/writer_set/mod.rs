#![allow(dead_code)]

pub mod error;
pub mod store;

use crate::crypto::{
    sign::{Keypair, PublicKey, Signature},
    Hash,
};
use sha3::{Digest, Sha3_256};
use std::collections::{hash_map, HashMap};
use std::{cell::Cell, fmt, iter::once};

///
/// A structure to keep track who who can sign modifications to the repository index.
/// A new entry in the writer set can only be done by someone already in the set.
///
#[derive(Clone, Debug)]
pub struct WriterSet {
    origin: Hash,
    entries: HashMap<Hash, Entry>,
}

impl WriterSet {
    pub fn generate() -> (Self, Keypair) {
        let origin = Keypair::generate();

        let entry = Entry::new(&origin.public, &origin);

        let ret = Self {
            origin: entry.hash,
            entries: once((entry.hash, entry)).collect(),
        };

        (ret, origin)
    }

    pub fn new_from_existing(origin_entry: &Entry) -> Option<Self> {
        if !origin_entry.is_origin() || !origin_entry.has_valid_signature() {
            return None;
        }

        Some(Self {
            origin: origin_entry.hash,
            entries: once((origin_entry.hash, origin_entry.clone())).collect(),
        })
    }

    pub fn origin_entry(&self) -> &Entry {
        self.entries
            .get(&self.origin)
            .expect("not found origin in WriterSet")
    }

    pub fn is_writer(&self, w: &PublicKey) -> bool {
        self.entries.values().any(|e| &e.writer == w)
    }

    pub fn add_writer(&mut self, writer: &PublicKey, added_by: &Keypair) -> Option<Entry> {
        self.make_entry(writer, added_by).map(|e| {
            self.entries.insert(e.hash, e.clone());
            e
        })
    }

    pub fn make_entry(&self, writer: &PublicKey, added_by: &Keypair) -> Option<Entry> {
        if !self.is_writer(&added_by.public) {
            return None;
        }
        Some(Entry::new(writer, added_by))
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Prepared entries can be added/inserted into the WriterSet. This machinery
    /// exists to avoid removing entries from WriterSet if entries fail to get written
    /// onto the disk.
    pub fn prepare_entry(&mut self, entry: Entry) -> Option<PreparedEntry<'_>> {
        if !self.is_writer(&entry.added_by) {
            return None;
        }

        let vacant = match self.entries.entry(entry.hash) {
            hash_map::Entry::Occupied(_) => return None,
            hash_map::Entry::Vacant(vacant) => vacant,
        };

        if !entry.has_valid_signature() {
            return None;
        }

        Some(PreparedEntry { vacant, entry })
    }
}

pub struct PreparedEntry<'a> {
    vacant: hash_map::VacantEntry<'a, Hash, Entry>,
    entry: Entry,
}

impl<'a> PreparedEntry<'a> {
    pub fn hash(&self) -> &Hash {
        self.vacant.key()
    }

    pub fn insert(self) {
        self.vacant.insert(self.entry);
    }
}

#[derive(Clone)]
pub struct Entry {
    writer: PublicKey,
    added_by: PublicKey,
    // Calculated from above.
    hash: Hash,
    // Calculated from the `hash` and the private key corresponding to `added_by`.
    signature: Signature,

    has_valid_hash: Cell<Option<bool>>,
    has_valid_signature: Cell<Option<bool>>,
}

impl Entry {
    fn new(writer: &PublicKey, added_by: &Keypair) -> Self {
        // XXX This is kinda silly, we hash the Entry and then the sign function hashes the hash
        // which it then signs. If the API gave us a way to retrieve the signed hash we wouldn't
        // have to hash twice. There is an (`sign_prehashed`) API that takes Sha512 (hasher, not
        // digest) as an argument, but it doesn't let us look at the digest.
        let hash = hash_entry(writer, &added_by.public);
        let signature = added_by.sign(hash.as_ref());

        Self {
            writer: *writer,
            added_by: added_by.public,
            hash,
            signature,
            has_valid_hash: Cell::new(Some(true)),
            has_valid_signature: Cell::new(Some(true)),
        }
    }

    pub fn is_valid(&self) -> bool {
        // This ensures hash is valid as well.
        self.has_valid_signature()
    }

    pub fn has_valid_hash(&self) -> bool {
        let b = self.has_valid_hash.get();
        match b {
            Some(b) => b,
            None => {
                let v = self.hash == hash_entry(&self.writer, &self.added_by);
                self.has_valid_hash.set(Some(v));
                v
            }
        }
    }

    pub fn has_valid_signature(&self) -> bool {
        if !self.has_valid_hash() {
            return false;
        }

        let b = self.has_valid_signature.get();
        match b {
            Some(b) => b,
            None => {
                let v = self.added_by.verify(self.hash.as_ref(), &self.signature);
                self.has_valid_signature.set(Some(v));
                v
            }
        }
    }

    pub fn is_origin(&self) -> bool {
        self.writer == self.added_by
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.hash)
    }
}

fn hash_entry(writer: &PublicKey, added_by: &PublicKey) -> Hash {
    let mut hasher = Sha3_256::new();

    hasher.update(b"OuiSync WriterSet Entry");
    hasher.update(writer);
    hasher.update(added_by);

    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity() {
        let (mut ws, alice) = WriterSet::generate();

        assert!(ws.is_writer(&alice.public));

        let bob = Keypair::generate();
        let carol = Keypair::generate();

        assert!(!ws.is_writer(&bob.public));
        assert!(ws.add_writer(&carol.public, &bob).is_none());
        assert!(ws.add_writer(&bob.public, &alice).is_some());
        assert!(ws.add_writer(&carol.public, &bob).is_some());
    }

    #[test]
    fn merge() {
        let (mut ws1, alice) = WriterSet::generate();
        let mut ws2 = ws1.clone();

        let bob = Keypair::generate();
        let carol = Keypair::generate();

        ws1.add_writer(&bob.public, &alice);
        ws2.add_writer(&carol.public, &alice);

        for ws2_entry in ws2.entries() {
            if let Some(e) = ws1.prepare_entry(ws2_entry.clone()) {
                e.insert()
            }
        }

        assert!(ws1.is_writer(&carol.public));

        for ws1_entry in ws1.entries() {
            if let Some(e) = ws2.prepare_entry(ws1_entry.clone()) {
                e.insert()
            }
        }
    }

    #[test]
    fn merge_incompatible() {
        let (mut ws1, alice) = WriterSet::generate();
        let (mut ws2, mallory) = WriterSet::generate();

        let bob = Keypair::generate();
        let carol = Keypair::generate();

        ws1.add_writer(&bob.public, &alice);
        ws2.add_writer(&carol.public, &mallory);

        for ws2_entry in ws2.entries() {
            assert!(ws1.prepare_entry(ws2_entry.clone()).is_none());
        }

        assert!(!ws1.is_writer(&carol.public));
    }
}
