use crate::crypto::{Digest, Hashable};
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use serde::{Deserialize, Serialize};

define_byte_array_wrapper! {
    /// BlobId is used to identify a blob in a directory
    #[derive(Serialize, Deserialize)]
    pub(crate) struct BlobId([u8; 32]);
}

impl BlobId {
    pub(crate) const ROOT: Self = Self([0; Self::SIZE]);
}

// Never generates `ROOT`
impl Distribution<BlobId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlobId {
        loop {
            let sample = BlobId(self.sample(rng));
            if sample != BlobId::ROOT {
                return sample;
            }
        }
    }
}

impl Hashable for BlobId {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state)
    }
}
