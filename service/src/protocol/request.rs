use crate::{file::FileHandle, repository::RepositoryHandle};
use ouisync::{
    crypto::PasswordSalt, AccessChange, AccessMode, LocalSecret, PeerAddr, SetLocalSecret,
    ShareToken, StorageSize,
};
use serde::{Deserialize, Serialize};
use state_monitor::MonitorId;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use super::{
    helpers::{self, Bytes},
    MessageId, MetadataEdit,
};

// The `Request` enum is auto-generated in `build.rs` from the `#[api]` annotated methods in `impl
// State` and `impl Service`.
include!(concat!(env!("OUT_DIR"), "/request.rs"));

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetworkDefaults {
    #[serde(with = "helpers::strs")]
    pub bind: Vec<PeerAddr>,
    pub port_forwarding_enabled: bool,
    pub local_discovery_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{AccessSecrets, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::Ipv4Addr;

    #[test]
    fn serialize() {
        let mut rng = StdRng::seed_from_u64(0);
        let secrets = AccessSecrets::Write(WriteSecrets::generate(&mut rng));

        let test_vectors = [
            (
                Request::NetworkAddUserProvidedPeers { addrs: vec![] },
                "81bf6e6574776f726b5f6164645f757365725f70726f76696465645f70656572739190",
            ),
            (
                Request::NetworkAddUserProvidedPeers {
                    addrs: vec![PeerAddr::Quic(SocketAddr::from((
                        Ipv4Addr::LOCALHOST,
                        12345,
                    )))]
                },
                "81bf6e6574776f726b5f6164645f757365725f70726f76696465645f70656572739191b4717569632f\
                 3132372e302e302e313a3132333435",
            ),
            (
                Request::NetworkBind { addrs: vec![PeerAddr::Quic(SocketAddr::from((
                    Ipv4Addr::UNSPECIFIED,
                    12345,
                )))]},
                "81ac6e6574776f726b5f62696e649191b2717569632f302e302e302e303a3132333435",
            ),
            (
                Request::NetworkGetLocalListenerAddrs,
                "d9206e6574776f726b5f6765745f6c6f63616c5f6c697374656e65725f6164647273",
            ),
            (
                Request::RepositoryCreate {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                "81b17265706f7369746f72795f63726561746597a3666f6fc0c0c0c3c2c2",
            ),
            (
                Request::RepositoryCreate {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(ShareToken::from(secrets)),
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                "81b17265706f7369746f72795f63726561746597a3666f6fc0c0d94568747470733a2f2f6f75697379\
                 6e632e6e65742f722341774967663238737a62495f4b7274376153654f6c4877427868594b4d633843\
                 775a30473050626c71783132693555c3c2c2",
            ),
        ];

        for (request, expected_encoded) in test_vectors {
            let encoded = rmp_serde::to_vec(&request).unwrap();
            assert_eq!(hex::encode(&encoded), expected_encoded, "{:?}", request);

            let decoded: Request = rmp_serde::from_slice(&encoded).unwrap();

            assert_eq!(decoded, request);
        }
    }
}
