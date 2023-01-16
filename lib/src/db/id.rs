use rand::{
    distributions::{Distribution, Standard},
    Rng,
};

define_byte_array_wrapper! {
    /// Database ID is a locally public random byte array that can be used by the apps using this
    /// library to identify the database file. For example, if the app stores passwords externally
    /// (e.g.  fingerprint biometric) it can do so under the DATABASE_ID as key. Note that the
    /// RepositoryId could - in theory - also be used for this purpose, but in case the
    /// database IDs are leaked to an adversary without the actual database files, the adversary
    /// will not be able to link the DATABASE_ID to a corresponding repository. For that reason
    /// database IDs may be preferable.
    pub struct DatabaseId([u8; 16 /* 128 bit */]);
}

impl Distribution<DatabaseId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> DatabaseId {
        DatabaseId(self.sample(rng))
    }
}
