mod mdns_common;
mod mdns_direct;
mod mdns_zeroconf;
mod poor_man;

use crate::network::{peer_addr::PeerPort, seen_peers::SeenPeer};
use state_monitor::StateMonitor;
use tokio::select;

pub struct LocalDiscovery {
    poor_man: Option<poor_man::LocalDiscovery>,
    mdns_direct: Option<mdns_direct::LocalDiscovery>,
    mdns_zeroconf: Option<mdns_zeroconf::LocalDiscovery>,
}

impl LocalDiscovery {
    // TODO: This function should initialize `poor_man` in a "legacy compatibility mode" where it
    // only listens to beacons from older ouisync versions but does not do any multicasting.
    // It should also initialize either `mdns_direct` or a daemon based mdns depending on whether
    // connection to a zeroconf deamon is successful.
    pub fn new(listener_port: PeerPort, monitor: Option<StateMonitor>) -> Self {
        // We still use the legacy local discovery mechanism but in the `ObserveOnly` mode to be
        // able to find older Ouisync versions which did not use mDNS.
        let poor_man =
            poor_man::LocalDiscovery::new(listener_port, monitor, poor_man::Mode::ObserveOnly);

        // The "direct" mDNS won't work on iOS because the OS won't let us do multicasting.
        // On macOS (and iOS) the Zeroconf/Bonjour implementation seems to work good so we can use
        // it there. Zeroconf/Avahi has a problem that sometimes (specially when working
        // discovering mdns_direct peers) it detects service removal only after about 1 hour. So
        // it's up to consideration whether Zeroconf/Avahi should be preferred over mdns_direct.
        let mdns_direct = mdns_direct::LocalDiscovery::new(listener_port).ok();

        let mdns_zeroconf = if mdns_direct.is_none() {
            Some(mdns_zeroconf::LocalDiscovery::new(listener_port))
        } else {
            None
        };

        Self {
            poor_man: Some(poor_man),
            mdns_direct: mdns_direct,
            mdns_zeroconf: mdns_zeroconf,
        }
    }

    pub fn new_poor_man(listener_port: PeerPort) -> Self {
        Self {
            poor_man: Some(poor_man::LocalDiscovery::new(
                listener_port,
                None,
                poor_man::Mode::ObserveAndSignal,
            )),
            mdns_direct: None,
            mdns_zeroconf: None,
        }
    }

    pub fn new_mdns_direct(listener_port: PeerPort) -> mdns_sd::Result<Self> {
        Ok(Self {
            poor_man: None,
            mdns_direct: Some(mdns_direct::LocalDiscovery::new(listener_port)?),
            mdns_zeroconf: None,
        })
    }

    pub fn new_mdns_zeroconf(listener_port: PeerPort) -> Self {
        Self {
            poor_man: None,
            mdns_direct: None,
            mdns_zeroconf: Some(mdns_zeroconf::LocalDiscovery::new(listener_port)),
        }
    }

    pub async fn recv(&mut self) -> SeenPeer {
        // Note on `future::pending` below: To keep the API simple, instead of propagating the
        // `None` we wait forever.  However, this happens only during runtime shutdown so in
        // practice we don't wait at all.

        let poor_man_recv = async {
            let Some(poor_man) = self.poor_man.as_mut() else {
                std::future::pending::<()>().await;
                unreachable!();
            };
            poor_man.recv().await
        };

        let mdns_direct_recv = async {
            let Some(mdns) = self.mdns_direct.as_mut() else {
                std::future::pending::<()>().await;
                unreachable!();
            };
            match mdns.recv().await {
                Some(peer) => peer,
                None => std::future::pending::<SeenPeer>().await,
            }
        };

        let mdns_zeroconf_recv = async {
            let Some(mdns) = self.mdns_zeroconf.as_mut() else {
                std::future::pending::<()>().await;
                unreachable!();
            };
            match mdns.recv().await {
                Some(peer) => peer,
                None => std::future::pending::<SeenPeer>().await,
            }
        };

        select! {
            peer = poor_man_recv => peer,
            peer = mdns_direct_recv => peer,
            peer = mdns_zeroconf_recv => peer,
        }
    }
}
