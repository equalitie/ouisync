use rand::{Rng, rngs::OsRng};
use ed25519_dalek::{
    Keypair,
    Signature,
    Signer,
    SECRET_KEY_LENGTH,
    PublicKey,
    SecretKey,
    Verifier
};
use crate::crypto::Hash;
use std::collections::{BTreeSet, HashMap};
use sha3::{Digest, Sha3_256};
use std::{fmt, iter::once};

//
// We want to ensure:
//
// 1. Entries can only be added by someone already in the set, or by the origin.
// 2. Entries can't be removed
// 3. Two sets with the same origin can be merged without write permissions
//
#[derive(Clone, Debug)]
pub struct WriterSet {
    origin: Hash,
    entries: HashMap<Hash, Entry>,
}

impl WriterSet {
    pub fn generate() -> (Self, Keypair) {
        let origin = generate_key_pair();

        let entry = Entry::new(&origin.public, &origin, BTreeSet::default());

        let ret = Self {
            origin: entry.hash,
            entries: once((entry.hash, entry)).collect::<HashMap<_,_>>(),
        };

        (ret, origin)
    }

    pub fn calculate_hash(&self) -> Hash {
        let maximals = self.find_maximal_entries();

        let mut hasher = Sha3_256::new();

        hasher.update((maximals.len() as u32).to_le_bytes());

        for max in maximals {
            hasher.update(max.as_ref());
        }

        hasher.finalize().into()
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
        Some(Entry::new(writer, added_by, self.find_maximal_entries()))
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    pub fn add_entry(&mut self, entry: Entry) -> bool {
        if self.entries.get(&entry.hash).is_some() {
            return false;
        }

        if !entry.has_valid_signature() {
            return false;
        }

        if !self.is_writer(&entry.added_by) {
            return false;
        }

        for child in &entry.children {
            if self.entries.get(&child).is_none() {
                return false;
            }
        }

        self.entries.insert(entry.hash, entry);

        true
    }

    fn find_maximal_entries(&self) -> BTreeSet<Hash> {
        // Using `as_bytes` because of this issue
        // https://github.com/dalek-cryptography/ed25519-dalek/issues/183
        let mut candidates = self.entries.iter().map(|(hash, entry)| {
            (entry.writer.as_bytes(), *hash)
        })
        .collect::<HashMap<_, _>>();

        for entry in self.entries.values() {
            candidates.remove(entry.added_by.as_bytes());
        }

        if candidates.is_empty() {
            once(self.origin).collect()
        } else {
            candidates.values().cloned().collect()
        }
    }
}

#[derive(Clone)]
pub struct Entry {
    writer: PublicKey,
    added_by: PublicKey,
    children: BTreeSet<Hash>,
    // Calculated from above.
    hash: Hash,
    // Calculated from the `hash` and the private key corresponding to `added_by`.
    signature: Signature,
}

impl Entry {
    fn new(writer: &PublicKey, added_by: &Keypair, children: BTreeSet<Hash>) -> Self {
        // XXX This is kinda silly, we hash the Entry and then the sign function hashes the hash
        // which it then signs. If the API gave us a way to retrieve the signed hash we wouldn't
        // have to hash twice. There is an (`sign_prehashed`) API that takes Sha512 (hasher, not
        // digest) as an argument, but it doesn't let us look at the digest.
        let hash = hash_entry(writer, &added_by.public, &children);
        let signature = added_by.sign(hash.as_ref());

        let entry = Self {
            writer: *writer,
            added_by: added_by.public,
            children,
            hash,
            signature,
        };

        entry
    }

    fn has_valid_signature(&self) -> bool {
        self.added_by.verify(self.hash.as_ref(), &self.signature).is_ok()
    }

    fn is_origin(&self) -> bool {
        return self.writer == self.added_by;
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.hash)
    }
}

fn hash_entry(writer: &PublicKey, added_by: &PublicKey, children: &BTreeSet<Hash>) -> Hash {
    let mut hasher = Sha3_256::new();
    
    hasher.update(b"OuiSync WriterSet Entry");
    hasher.update(writer.as_bytes());
    hasher.update(added_by.as_bytes());
    hasher.update((children.len() as u32).to_le_bytes());
    
    for child in children.iter() {
        hasher.update(child.as_ref());
    }
    
    hasher.finalize().into()
}

fn generate_key_pair() -> Keypair {
    // TODO: Not using Keypair::generate because `ed25519_dalek` uses an incompatible version
    // of the `rand` dependency.
    // https://stackoverflow.com/questions/65562447/the-trait-rand-corecryptorng-is-not-implemented-for-osrng
    // https://github.com/dalek-cryptography/ed25519-dalek/issues/162
    let mut bytes = [0u8; SECRET_KEY_LENGTH];
    OsRng{}.fill(&mut bytes[..]);
    let sk = SecretKey::from_bytes(&bytes).unwrap();
    let pk = (&sk).into();
    Keypair { secret: sk, public: pk }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity() {
        let (mut ws, alice) = WriterSet::generate();

        assert!(ws.is_writer(&alice.public));

        let bob = generate_key_pair();
        let carol = generate_key_pair();

        assert!(!ws.is_writer(&bob.public));
        assert!(ws.add_writer(&carol.public, &bob).is_none());
        assert!(ws.add_writer(&bob.public, &alice).is_some());
        assert!(ws.add_writer(&carol.public, &bob).is_some());
    }

    #[test]
    fn merge() {
        let (mut ws1, alice) = WriterSet::generate();
        let mut ws2 = ws1.clone();

        let bob = generate_key_pair();
        let carol = generate_key_pair();

        ws1.add_writer(&bob.public, &alice);
        ws2.add_writer(&carol.public, &alice);

        for ws2_entry in ws2.entries() {
            ws1.add_entry(ws2_entry.clone());
        }

        assert!(ws1.is_writer(&carol.public));

        for ws1_entry in ws1.entries() {
            ws2.add_entry(ws1_entry.clone());
        }

        assert_eq!(ws1.calculate_hash(), ws2.calculate_hash());
    }

    #[test]
    fn merge_incompatible() {
        let (mut ws1, alice) = WriterSet::generate();
        let (mut ws2, mallory) = WriterSet::generate();

        let bob = generate_key_pair();
        let carol = generate_key_pair();

        ws1.add_writer(&bob.public, &alice);
        ws2.add_writer(&carol.public, &mallory);

        for ws2_entry in ws2.entries() {
            assert!(!ws1.add_entry(ws2_entry.clone()));
        }

        assert!(!ws1.is_writer(&carol.public));
    }
}
