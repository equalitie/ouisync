mod mdns_direct;
pub mod poor_man;

use crate::network::{peer_addr::PeerPort, seen_peers::SeenPeer};
use state_monitor::StateMonitor;
use tokio::select;

pub struct LocalDiscovery {
    poor_man: Option<poor_man::LocalDiscovery>,
    mdns_direct: Option<mdns_direct::LocalDiscovery>,
}

impl LocalDiscovery {
    // TODO: This function should initialize `poor_man` in a "legacy compatibility mode" where it
    // only listens to beacons from older ouisync versions but does not do any multicasting.
    // It should also initialize either `mdns_direct` or a daemon based mdns depending on whether
    // connection to a zeroconf deamon is successful.
    pub fn new(listener_port: PeerPort, monitor: Option<StateMonitor>) -> Self {
        Self {
            poor_man: Some(poor_man::LocalDiscovery::new(listener_port, monitor)),
            mdns_direct: None,
        }
    }

    pub fn new_mdns_direct(listener_port: PeerPort) -> Self {
        Self {
            poor_man: None,
            mdns_direct: Some(mdns_direct::LocalDiscovery::new(listener_port)),
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
            let Some(mdns_direct) = self.mdns_direct.as_mut() else {
                std::future::pending::<()>().await;
                unreachable!();
            };
            match mdns_direct.recv().await {
                Some(peer) => peer,
                None => std::future::pending::<SeenPeer>().await,
            }
        };

        select! {
            peer = poor_man_recv => peer,
            peer = mdns_direct_recv => peer,
        }
    }
}
