use crate::{error::Error, state::State};
use ouisync_lib::{network, ShareToken};

/// Returns the access mode of the given share token.
pub(crate) fn mode(token: ShareToken) -> u8 {
    token.access_mode().into()
}

/// Returns the info-hash of the repository corresponding to the share token formatted as hex
/// string.
pub(crate) fn info_hash(token: ShareToken) -> String {
    hex::encode(network::repository_info_hash(token.id()).as_ref())
}

pub(crate) fn suggested_name(token: ShareToken) -> String {
    token.suggested_name().to_owned()
}

/// Check if the repository is mirrored on the given server.
pub(crate) async fn mirror_exists(
    state: &State,
    token: ShareToken,
    host: &str,
) -> Result<bool, Error> {
    let config = state.get_remote_client_config().await?;
    Ok(ouisync_bridge::repository::mirror_exists(token.id(), config, host).await?)
}
