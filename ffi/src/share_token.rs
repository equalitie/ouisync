use crate::repository::access_mode_to_num;
use ouisync_lib::{network, ShareToken};

/// Returns the access mode of the given share token.
pub(crate) fn mode(token: ShareToken) -> u8 {
    access_mode_to_num(token.access_mode())
}

/// Returns the info-hash of the repository corresponding to the share token formatted as hex
/// string.
pub(crate) fn info_hash(token: ShareToken) -> String {
    hex::encode(network::repository_info_hash(token.id()).as_ref())
}

pub(crate) fn suggested_name(token: ShareToken) -> String {
    token.suggested_name().into_owned()
}

pub(crate) fn encode(token: ShareToken) -> Vec<u8> {
    let mut buffer = Vec::new();
    token.encode(&mut buffer);
    buffer
}

pub(crate) fn decode(bytes: Vec<u8>) -> Option<ShareToken> {
    ShareToken::decode(&bytes).ok()
}
