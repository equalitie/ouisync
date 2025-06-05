use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Protocol {
    Tcp,
    Udp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Tcp => write!(f, "TCP"),
            Self::Udp => write!(f, "UDP"),
        }
    }
}

/// Following are convenience methods copy pasted from nightly-only experimental rust API.
///
/// https://github.com/rust-lang/rust/issues/27709
// TODO: Get rid of the below once `IpAddr::is_global` is in stable API.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const fn is_global(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_ipv4(ip),
        IpAddr::V6(ip) => is_global_ipv6(ip),
    }
}

const fn is_global_ipv4(ip: &Ipv4Addr) -> bool {
    // check if this address is 192.0.0.9 or 192.0.0.10. These addresses are the only two
    // globally routable addresses in the 192.0.0.0/24 range.
    if u32::from_be_bytes(ip.octets()) == 0xc0000009
        || u32::from_be_bytes(ip.octets()) == 0xc000000a
    {
        return true;
    }
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && !is_shared(ip)
        // addresses reserved for future protocols (`192.0.0.0/24`)
        && !(ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
        && !is_reserved(ip)
        && !is_benchmarking(ip)
        // Make sure the address is not in 0.0.0.0/8
        && ip.octets()[0] != 0
}

const fn is_shared(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] == 100 && (ip.octets()[1] & 0b1100_0000 == 0b0100_0000)
}

pub const fn is_reserved(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] & 240 == 240 && !ip.is_broadcast()
}

const fn is_global_ipv6(ip: &Ipv6Addr) -> bool {
    match multicast_scope_ipv6(ip) {
        Some(Ipv6MulticastScope::Global) => true,
        None => is_unicast_global_ipv6(ip),
        _ => false,
    }
}

pub const fn is_benchmarking(ip: &Ipv4Addr) -> bool {
    ip.octets()[0] == 198 && (ip.octets()[1] & 0xfe) == 18
}

const fn is_unicast(ip: &Ipv6Addr) -> bool {
    !ip.is_multicast()
}

const fn is_unicast_global_ipv6(ip: &Ipv6Addr) -> bool {
    is_unicast(ip)
        && !ip.is_loopback()
        && !ip.is_unicast_link_local()
        && !ip.is_unique_local()
        && !ip.is_unspecified()
        && !is_documentation(ip)
}

pub const fn is_documentation(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] == 0x2001) && (ip.segments()[1] == 0xdb8)
}

enum Ipv6MulticastScope {
    /// Interface-Local scope.
    InterfaceLocal,
    /// Link-Local scope.
    LinkLocal,
    /// Realm-Local scope.
    RealmLocal,
    /// Admin-Local scope.
    AdminLocal,
    /// Site-Local scope.
    SiteLocal,
    /// Organization-Local scope.
    OrganizationLocal,
    /// Global scope.
    Global,
}

const fn multicast_scope_ipv6(ip: &Ipv6Addr) -> Option<Ipv6MulticastScope> {
    if ip.is_multicast() {
        match ip.segments()[0] & 0x000f {
            1 => Some(Ipv6MulticastScope::InterfaceLocal),
            2 => Some(Ipv6MulticastScope::LinkLocal),
            3 => Some(Ipv6MulticastScope::RealmLocal),
            4 => Some(Ipv6MulticastScope::AdminLocal),
            5 => Some(Ipv6MulticastScope::SiteLocal),
            8 => Some(Ipv6MulticastScope::OrganizationLocal),
            14 => Some(Ipv6MulticastScope::Global),
            _ => None,
        }
    } else {
        None
    }
}
