use std::net::SocketAddr;

use super::ip;

/// Filters out certain types of socket addresses which are unlikely to belong to valid peers (e.g.,
/// addresses from reserved ranges, etc...)
///
/// Using non-default filter typically only makes sense for testing (e.g., when using
/// [patchbay](https://crates.io/crates/patchbay) which allocates IPs from the "benchmarking"
/// range).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AddrFilter {
    /// Whether
    /// [benchmarking](https://doc.rust-lang.org/beta/std/net/struct.Ipv4Addr.html#method.is_benchmarking)
    /// IPv4 addresses are allowed. Default is `false`.
    pub allow_benchmarking_v4: bool,
}

impl AddrFilter {
    pub fn allow_benchmarking_v4(self) -> Self {
        Self {
            allow_benchmarking_v4: true,
        }
    }
}

#[expect(clippy::derivable_impls)] // we want to be explicit here
impl Default for AddrFilter {
    fn default() -> Self {
        Self {
            allow_benchmarking_v4: false,
        }
    }
}

impl AddrFilter {
    /// Apply the filter to the address. Returns whether the address passes the filter.
    pub(super) fn apply(&self, addr: &SocketAddr) -> bool {
        if addr.port() == 0 || addr.port() == 1 {
            return false;
        }

        match addr {
            SocketAddr::V4(addr) => {
                let ip_addr = addr.ip();
                if ip_addr.octets()[0] == 0 {
                    return false;
                }
                if (!self.allow_benchmarking_v4 && ip::is_benchmarking(ip_addr))
                    || ip::is_reserved(ip_addr)
                    || ip_addr.is_broadcast()
                    || ip_addr.is_documentation()
                {
                    return false;
                }
            }
            SocketAddr::V6(addr) => {
                let ip_addr = addr.ip();

                if ip_addr.is_multicast()
                    || ip_addr.is_unspecified()
                    || ip::is_documentation(ip_addr)
                {
                    return false;
                }
            }
        }

        true
    }
}
