mod share_token;

pub use self::share_token::ShareToken;

use crate::{
    crypto::{sign, Hashable, SecretKey},
    repository::RepositoryId,
};

pub(crate) struct AccessSecrets {
    pub write_key: sign::SecretKey,
    // The read_key is calculated as a hash of the write_key.
    pub read_key: SecretKey,
    // The public part corresponding to write_key
    pub repo_id: RepositoryId,
}

impl AccessSecrets {
    pub fn generate() -> Self {
        let keypair = sign::Keypair::generate();

        AccessSecrets {
            write_key: keypair.secret,
            read_key: keypair.public.as_ref().hash().into(),
            repo_id: keypair.public.into(),
        }
    }
}
