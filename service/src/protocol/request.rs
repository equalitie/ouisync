use crate::{file::FileHandle, repository::RepositoryHandle};
use ouisync::{
    crypto::{Password, PasswordSalt},
    AccessChange, AccessMode, LocalSecret, PeerAddr, SetLocalSecret, ShareToken, StorageSize,
};
use ouisync_macros::api;
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

/// Default network parameters
#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[api]
pub struct NetworkDefaults {
    #[serde(with = "helpers::strs")]
    /// Default addresses to bind to
    pub bind: Vec<PeerAddr>,
    /// Is port forwarding (UPnP) enabled by default?
    pub port_forwarding_enabled: bool,
    /// Is local discovery enabled by default?
    pub local_discovery_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{AccessSecrets, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn serialize_deserialize_msgpack() {
        use rmp::encode::*;

        let mut rng = StdRng::seed_from_u64(0);
        let secrets = AccessSecrets::Write(WriteSecrets::generate(&mut rng));

        let test_vectors = [
            (Request::SessionGetCurrentProtocolVersion, {
                let mut out = Vec::new();
                write_str(&mut out, "SessionGetCurrentProtocolVersion").unwrap();
                out
            }),
            (
                Request::FileClose {
                    file: FileHandle::from_raw(1),
                },
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "FileClose").unwrap();
                    write_array_len(&mut out, 1).unwrap();
                    write_uint(&mut out, 1).unwrap();
                    out
                },
            ),
            (
                Request::SessionCreateRepository {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "SessionCreateRepository").unwrap();
                    write_array_len(&mut out, 7).unwrap();
                    write_str(&mut out, "foo").unwrap();
                    write_nil(&mut out).unwrap();
                    write_nil(&mut out).unwrap();
                    write_nil(&mut out).unwrap();
                    write_bool(&mut out, true).unwrap();
                    write_bool(&mut out, false).unwrap();
                    write_bool(&mut out, false).unwrap();
                    out
                },
            ),
            (
                Request::SessionCreateRepository {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(ShareToken::from(secrets.clone())),
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "SessionCreateRepository").unwrap();
                    write_array_len(&mut out, 7).unwrap();
                    write_str(&mut out, "foo").unwrap();
                    write_nil(&mut out).unwrap();
                    write_nil(&mut out).unwrap();
                    write_str(&mut out, &ShareToken::from(secrets.clone()).to_string()).unwrap();
                    write_bool(&mut out, true).unwrap();
                    write_bool(&mut out, false).unwrap();
                    write_bool(&mut out, false).unwrap();
                    out
                },
            ),
        ];

        for (input, expected) in test_vectors {
            let s = rmp_serde::to_vec(&input).unwrap();
            assert_eq!(s, expected, "{:?}", input);

            let d: Request = rmp_serde::from_slice(&s).unwrap();
            assert_eq!(d, input, "{:?}", input);
        }
    }
}
