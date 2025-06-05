use std::{net::SocketAddr, path::PathBuf};

use ouisync::{DeviceId, PeerAddr};

use crate::{config_store::ConfigKey, network::PexConfig};

pub(crate) const BIND_KEY: ConfigKey<Vec<PeerAddr>> =
    ConfigKey::new("bind", "Addresses to bind the network listeners to");

pub(crate) const BIND_METRICS_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("bind_metrics", "Addresses to bind the metrics endpoint to");

pub(crate) const DEFAULT_QUOTA_KEY: ConfigKey<u64> =
    ConfigKey::new("default_quota", "Default storage quota");

pub(crate) const DEFAULT_BLOCK_EXPIRATION_MILLIS: ConfigKey<u64> = ConfigKey::new(
    "default_block_expiration",
    "Default time in milliseconds when blocks start to expire if not used",
);

pub(crate) const DEVICE_ID_KEY: ConfigKey<DeviceId> = ConfigKey::new(
    "device_id",
    "The value stored in this file is the device ID. It is uniquelly generated for each device\n\
     and its only purpose is to detect when a database has been migrated from one device to\n\
     another.\n\
     \n\
     * When a database is migrated, the safest option is to NOT migrate this file with it. *\n\
     \n\
     However, the user may chose to *move* this file alongside the database. In such case it is\n\
     important to ensure the same device ID is never used by a writer replica concurrently from\n\
     more than one location. Doing so will likely result in data loss.\n\
     \n\
     Device ID is never used in construction of network messages and thus can't be used for peer\n\
     identification.",
);

pub(crate) const LAST_USED_TCP_V4_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_tcp_v4_port", LAST_USED_TCP_PORT_COMMENT);

pub(crate) const LAST_USED_TCP_V6_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_tcp_v6_port", LAST_USED_TCP_PORT_COMMENT);

pub(crate) const LAST_USED_UDP_V4_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v4", LAST_USED_UDP_PORT_COMMENT);

pub(crate) const LAST_USED_UDP_V6_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v6", LAST_USED_UDP_PORT_COMMENT);

const LAST_USED_TCP_PORT_COMMENT: &str =
    "The value stored in this file is the last used TCP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";

// Intentionally not being explicity about DHT as eventually this port shall be shared with QUIC.
const LAST_USED_UDP_PORT_COMMENT: &str =
    "The value stored in this file is the last used UDP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";

pub(crate) const LOCAL_DISCOVERY_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("local_discovery_enabled", "Enable local discovery");

pub(crate) const MOUNT_DIR_KEY: ConfigKey<PathBuf> =
    ConfigKey::new("mount_dir", "Repository mount directory");

pub(crate) const PEERS_KEY: ConfigKey<Vec<PeerAddr>> = ConfigKey::new(
    "peers",
    "List of peers to connect to in addition to the ones found by various discovery mechanisms\n\
     (e.g. DHT)",
);

pub(crate) const PEX_KEY: ConfigKey<PexConfig> =
    ConfigKey::new("pex", "Peer exchange configuration");

pub(crate) const PORT_FORWARDING_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("port_forwarding_enabled", "Enable port forwarding / UPnP");

pub(crate) const STORE_DIR_KEY: ConfigKey<PathBuf> =
    ConfigKey::new("store_dir", "Repository storage directory");

pub(crate) const DEFAULT_REPOSITORY_EXPIRATION_KEY: ConfigKey<u64> = ConfigKey::new(
    "default_repository_expiration",
    "Default time in milliseconds after repository is deleted if all its blocks expired",
);
