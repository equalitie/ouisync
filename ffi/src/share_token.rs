use crate::repository::access_mode_to_num;
use ouisync_lib::{network, Result, ShareToken};
use std::str::FromStr;

/// Returns the access mode of the given share token.
pub(crate) fn mode(token: String) -> Result<u8> {
    Ok(access_mode_to_num(
        ShareToken::from_str(&token)?.access_mode(),
    ))
}

/// Returns the info-hash of the repository corresponding to the share token formatted as hex
/// string.
pub(crate) fn info_hash(token: String) -> Result<String> {
    Ok(hex::encode(
        network::repository_info_hash(ShareToken::from_str(&token)?.id()).as_ref(),
    ))
}

pub(crate) fn suggested_name(token: String) -> Result<String> {
    Ok(ShareToken::from_str(&token)?.suggested_name().into_owned())
}

pub(crate) fn normalize(token: String) -> Result<String> {
    Ok(ShareToken::from_str(&token)?.to_string())
}

pub(crate) fn encode(token: String) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    ShareToken::from_str(&token)?.encode(&mut buffer);
    Ok(buffer)
}

pub(crate) fn decode(bytes: Vec<u8>) -> Result<String> {
    Ok(ShareToken::decode(&bytes)?.to_string())
}
