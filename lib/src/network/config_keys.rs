use crate::config::ConfigKey;

pub(crate) const LAST_USED_TCP_V4_PORT_KEY: ConfigKey<u16> = ConfigKey::new(
    "last_used_tcp_v4_port",
    "The value stored in this file is the last used TCP IPv4 port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.\n\
     \n\
     The value is not used when the user specifies a non zero port in the --bind option on the\n\
     command line. However, it may still be overwritten.",
);

pub(crate) const LAST_USED_TCP_V6_PORT_KEY: ConfigKey<u16> = ConfigKey::new(
    "last_used_tcp_v6_port",
    "The value stored in this file is the last used TCP IPv6 port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.\n\
     \n\
     The value is not used when the user specifies a non zero port in the --bind option on the\n\
     command line. However, it may still be overwritten.",
);

pub(crate) const LAST_USED_UDP_PORT_V4_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v4", LAST_USED_UDP_PORT_COMMENT);

pub(crate) const LAST_USED_UDP_PORT_V6_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v6", LAST_USED_UDP_PORT_COMMENT);

// Intentionally not being explicity about DHT as eventually this port shall be shared with QUIC.
const LAST_USED_UDP_PORT_COMMENT: &str =
    "The value stored in this file is the last used UDP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";
