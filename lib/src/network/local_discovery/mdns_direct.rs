//!
//! Local discovery using mDNS, directly doing UDP multicasting.  Directly, as opposed to
//! connecting to a Zeroconf daemon (Bonjour, Avahi) which does the actual multicasting.
//!
//! Notably on iOS we can't do multicasting without explicit permission granted from Apple.
//!
//! This local discovery should be used on devices where Zeroconf is not available.
//!
use super::mdns_common;
use crate::network::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::SeenPeer,
};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use scoped_task::ScopedJoinHandle;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::Instrument;

pub struct LocalDiscovery {
    daemon: ServiceDaemon,
    peer_rx: mpsc::UnboundedReceiver<SeenPeer>,
    _worker: Option<ScopedJoinHandle<()>>,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort) -> Self {
        let span = tracing::info_span!("mDNS-zeroconf");

        // TODO: Unwraps and expects
        let daemon = ServiceDaemon::new().expect("Failed to create daemon");

        let service_type = match listener_port {
            PeerPort::Tcp(_) => "_ouisync._tcp.local.",
            PeerPort::Quic(_) => "_ouisync._udp.local.",
        };

        // TODO: We could use this to distribute protocol version number.
        let properties: [(&str, &str); 0] = [];

        let instance_name = mdns_common::generate_instance_name();

        tracing::debug!(parent: &span, "Service name of this replica: {instance_name:?}");

        let service_info = ServiceInfo::new(
            service_type,                       // service_type
            &instance_name,                     // instance_name
            &format!("{instance_name}.local."), // host_name
            (),                                 // ip (auto detect)
            listener_port.number(),             // port
            &properties[..],                    // properties
        )
        // Unwrap should be OK because nothing that depends on input to `LocalDiscovery::new`
        // should cause `ServiceInfo::new` to fail.
        .unwrap()
        .enable_addr_auto();

        daemon
            .register(service_info)
            .expect("Failed to register service");

        let receiver = daemon.browse(service_type).expect("Failed to browse");

        let (peer_tx, peer_rx) = mpsc::unbounded_channel();

        let worker = scoped_task::spawn(
            async move {
                let mut seen_peers = mdns_common::SeenMdnsPeers::new();

                'topmost_loop: while let Ok(event) = receiver.recv_async().await {
                    match event {
                        ServiceEvent::SearchStarted(_) => {}
                        ServiceEvent::SearchStopped(_) => {}
                        ServiceEvent::ServiceFound(_service, _domain) => {}
                        ServiceEvent::ServiceResolved(info) => {
                            // Avoid connecting to self.
                            if info.get_fullname().starts_with(&instance_name) {
                                continue;
                            }

                            for address in info.get_addresses() {
                                let address = SocketAddr::new(*address, info.get_port());

                                let address = match listener_port {
                                    PeerPort::Tcp(_) => PeerAddr::Tcp(address),
                                    PeerPort::Quic(_) => PeerAddr::Quic(address),
                                };

                                if let Some(seen_peer) =
                                    seen_peers.insert(info.get_fullname().to_owned(), address)
                                {
                                    if peer_tx.send(seen_peer).is_err() {
                                        break 'topmost_loop;
                                    }
                                }
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            seen_peers.remove(fullname);
                        }
                    }
                }

                tracing::info!("Runner finished");
            }
            .instrument(span),
        );

        Self {
            daemon,
            peer_rx,
            _worker: Some(worker),
        }
    }

    pub async fn recv(&mut self) -> Option<SeenPeer> {
        self.peer_rx.recv().await
    }

    fn shutdown_daemon(&self) {
        let max_attempts = 3;

        for attempt in 0..max_attempts {
            match self.daemon.shutdown() {
                Ok(_) => break,
                // From docs:
                //   https://docs.rs/mdns-sd/latest/mdns_sd/struct.ServiceDaemon.html#method.shutdown
                //
                //   When an error is returned, the caller should retry only when the error is
                //   Error::Again, otherwise should log and move on.
                //
                // It is not clear though how many times shutting down whould be attempted and
                // whether there should be a delay between the attempts.
                Err(mdns_sd::Error::Again) => {
                    if attempt == max_attempts - 1 {
                        tracing::warn!("Failed to shutdown mDNS-direct daemon: Error::Again");
                    }
                }
                Err(error) => {
                    tracing::warn!("Failed to shutdown mDNS-direct daemon: {error:?}");
                    break;
                }
            }
        }
    }
}

impl Drop for LocalDiscovery {
    fn drop(&mut self) {
        self.shutdown_daemon();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_test() {
        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let foo_addr = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());

            table.insert(foo_name.clone(), foo_addr.clone());

            assert_eq!(table.remove(foo_name), [foo_addr].into());
        }
        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let foo_addr1 = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());
            let foo_addr2 = PeerAddr::Quic(([192, 168, 1, 3], 1000).into());

            table.insert(foo_name.clone(), foo_addr1.clone());
            table.insert(foo_name.clone(), foo_addr2.clone());

            assert_eq!(table.remove(foo_name), [foo_addr1, foo_addr2].into());
        }

        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let bar_name = "bar".to_owned();
            let addr = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());

            table.insert(foo_name.clone(), addr.clone());
            table.insert(bar_name.clone(), addr.clone());

            assert_eq!(table.remove(foo_name), [].into());
            assert_eq!(table.remove(bar_name), [addr].into());
        }
    }
}
