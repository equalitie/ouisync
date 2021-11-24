use rand::{
    distributions::{Distribution, Standard},
    Rng,
};

define_byte_array_wrapper! {
    /// BlobId is used to identify a blob in a directory
    pub(crate) struct BlobId([u8; 32]);
}

impl BlobId {
    pub(crate) const ZERO: Self = Self([0; Self::SIZE]);
}

// Never generates `ZERO`
impl Distribution<BlobId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlobId {
        loop {
            let sample = BlobId(self.sample(rng));
            if sample != BlobId::ZERO {
                return sample;
            }
        }
    }
}
