use zeroize::Zeroize;

// TODO: Using scrypt because argon2 v0.3.2 did not compile.
use scrypt::{
    password_hash::{
        self,
        rand_core::{OsRng, RngCore},
    },
    scrypt, Params,
};

const SALT_LEN: usize = password_hash::Salt::RECOMMENDED_LENGTH;
const KEY_LEN: usize = 32;

pub struct MasterKey([u8; KEY_LEN]);
pub type Salt = [u8; SALT_LEN];

impl MasterKey {
    pub fn derive(user_password: &str, salt: &Salt) -> Self {
        let mut key = [0; KEY_LEN];
        scrypt(
            user_password.as_bytes(),
            salt,
            &Params::recommended(),
            &mut key,
        );
        Self(key)
    }

    pub fn generate_salt() -> Salt {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        salt
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize()
    }
}
