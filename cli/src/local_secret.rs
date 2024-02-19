use ouisync_lib::crypto::{cipher::SecretKey, Password};
use rand::{CryptoRng, Rng};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalSecret {
    Password(Password),
    SecretKey(SecretKey),
}

impl LocalSecret {
    /// Generates random master secret containing a secret key.
    pub fn random() -> Self {
        Self::SecretKey(SecretKey::random())
    }

    /// Generates random master secret containing a secret key using the provided RNG.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        Self::SecretKey(SecretKey::generate(rng))
    }
}

//#[cfg(test)]
//mod tests {
//    use super::*;
//
//    #[test]
//    fn serialize_deserialize_bincode() {
//        let orig = LocalSecret::Password("mellon".to_string().into());
//        let expected_serialized_hex = "0000000006000000000000006d656c6c6f6e";
//
//        let serialized = bincode::serialize(&orig).unwrap();
//        assert_eq!(hex::encode(&serialized), expected_serialized_hex);
//
//        let deserialized: LocalSecret = bincode::deserialize(&serialized).unwrap();
//        assert_eq!(&deserialized, &orig);
//    }
//
//    #[test]
//    fn serialize_deserialize_msgpack() {
//        let orig = LocalSecret::Password("mellon".to_string().into());
//        let expected_serialized_hex = "81a870617373776f7264a66d656c6c6f6e";
//
//        let serialized = rmp_serde::to_vec(&orig).unwrap();
//        assert_eq!(hex::encode(&serialized), expected_serialized_hex);
//
//        let deserialized: LocalSecret = rmp_serde::from_slice(&serialized).unwrap();
//        assert_eq!(&deserialized, &orig);
//    }
//}
