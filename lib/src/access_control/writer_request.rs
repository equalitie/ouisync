use super::share_token;
use crate::{crypto::sign, repository::SecretRepositoryId};

pub struct WriterRequest {
    pub(super) id: SecretRepositoryId,
    pub(super) giver_pk: sign::PublicKey,
    pub(super) recipient_pk: sign::PublicKey,
    pub(super) recipient_pk_signature: sign::Signature,
    pub(super) nonce_pk: sign::PublicKey,
    pub(super) nonce_pk_signature: sign::Signature,
}

impl WriterRequest {
    pub fn validate(&self) -> bool {
        let message = share_token::signature_material(&self.id, &self.nonce_pk);
        if !self.giver_pk.verify(&message, &self.nonce_pk_signature) {
            return false;
        }

        let message = share_token::signature_material(&self.id, &self.recipient_pk);
        if !self.nonce_pk.verify(&message, &self.recipient_pk_signature) {
            return false;
        }

        true
    }
}
