use tokio::{sync::mpsc, time::sleep};

use std::{collections::HashSet, net::Ipv4Addr, time::Duration};

// Network interfaces may change at runtime (especially on mobile devices, but on desktops as
// well). This code helps us track such changes.

const INTERFACE_REFRESH_DELAY: Duration = Duration::from_secs(5);

pub(crate) enum InterfaceChange {
    Added(HashSet<Ipv4Addr>),
    Removed(HashSet<Ipv4Addr>),
}

// Tells us when a network interface has been added or removed.
pub(crate) fn watch_ipv4_multicast_interfaces() -> mpsc::Receiver<InterfaceChange> {
    let (tx, rx) = mpsc::channel(1);

    tokio::spawn(async move {
        let mut seen_interfaces = HashSet::new();

        loop {
            let found_interfaces = find_ipv4_multicast_interfaces().await;

            let to_remove = seen_interfaces
                .difference(&found_interfaces)
                .cloned()
                .collect::<HashSet<_>>();

            seen_interfaces.retain(|e| !to_remove.contains(e));

            if !to_remove.is_empty() && tx.send(InterfaceChange::Removed(to_remove)).await.is_err()
            {
                // Channel was closed.
                return;
            }

            let to_add = found_interfaces
                .difference(&seen_interfaces)
                .cloned()
                .collect::<HashSet<_>>();

            for new in &to_add {
                seen_interfaces.insert(*new);
            }

            if !to_add.is_empty() && tx.send(InterfaceChange::Added(to_add)).await.is_err() {
                // Channel was closed.
                return;
            }

            sleep(INTERFACE_REFRESH_DELAY).await;
        }
    });

    rx
}

async fn find_ipv4_multicast_interfaces() -> HashSet<Ipv4Addr> {
    match tokio::task::spawn_blocking(find_ipv4_multicast_interfaces_sync).await {
        Ok(interfaces) => interfaces,
        Err(_) => HashSet::new(),
    }
}

#[cfg(target_family = "unix")]
fn find_ipv4_multicast_interfaces_sync() -> HashSet<Ipv4Addr> {
    use std::net::SocketAddrV4;

    let mut ret = HashSet::new();

    // This may be blocking
    let addrs = match nix::ifaddrs::getifaddrs() {
        Ok(addr) => addr,
        Err(_) => return ret,
    };

    for ifaddr in addrs {
        use nix::net::if_::InterfaceFlags;

        if !ifaddr.flags.contains(InterfaceFlags::IFF_MULTICAST) {
            continue;
        }

        if let Some(addr) = ifaddr.address.and_then(|addr| {
            let addr: Option<SocketAddrV4> = addr.as_sockaddr_in().map(|addr| (*addr).into());
            Some(*addr?.ip())
        }) {
            ret.insert(addr);
        }
    }

    ret
}

#[cfg(target_family = "windows")]
fn find_ipv4_multicast_interfaces_sync() -> HashSet<Ipv4Addr> {
    let mut ret = HashSet::new();

    use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};

    let network_interfaces = match NetworkInterface::show() {
        Ok(network_interfaces) => network_interfaces,
        Err(err) => {
            tracing::warn!("Failed to retrieve network interfaces: {:?}", err);
            return HashSet::new();
        }
    };

    for itf in network_interfaces.iter() {
        if let Some(addr) = itf.addr {
            match addr {
                Addr::V4(addr) => {
                    // XXX: We really want multicast, but the `network_interface` library doesn't
                    // seem to expose that flag.
                    if addr.broadcast.is_some() {
                        ret.insert(addr.ip);
                    }
                }
                Addr::V6(_addr) => {}
            }
        }
    }

    ret
}
