define_random_id! {
    /// BlobId is used to identify a blob in a directory
    pub struct BlobId([u8; 32]);
}

impl BlobId {
    pub fn zero() -> Self {
        BlobId([0; 32])
    }
}
