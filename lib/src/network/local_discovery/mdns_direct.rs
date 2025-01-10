//!
//! Local discovery using mDNS, directly doing UDP multicasting.  Directly, as opposed to
//! connecting to a Zeroconf daemon (Bonjour, Avahi) which does the actual multicasting.
//!
//! Notably on iOS we can't do multicasting without explicit permission granted from Apple.
//!
//! This local discovery should be used on devices where Zeroconf is not available.
//!
use crate::network::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::{SeenPeer, SeenPeers},
};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};
use tokio::sync::mpsc;
use tracing::Instrument;

pub struct LocalDiscovery {
    daemon: ServiceDaemon,
    peer_rx: mpsc::UnboundedReceiver<SeenPeer>,
    _worker: Option<ScopedJoinHandle<()>>,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort) -> Self {
        // TODO: Unwraps and expects
        let daemon = ServiceDaemon::new().expect("Failed to create daemon");

        let service_type = match listener_port {
            PeerPort::Tcp(_) => "_ouisync._tcp.local.",
            PeerPort::Quic(_) => "_ouisync._udp.local.",
        };

        // TODO: We could use this to distribute protocol version number.
        let properties: [(&str, &str); 0] = [];

        let instance_name = generate_instance_name();

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
                let seen_peers = SeenPeers::new();
                let mut table = Table::new();

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

                                table.insert(info.get_fullname().to_owned(), address);

                                if let Some(seen_peer) = seen_peers.insert(address) {
                                    if peer_tx.send(seen_peer).is_err() {
                                        break 'topmost_loop;
                                    }
                                }
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            for addr in table.remove(fullname) {
                                seen_peers.remove(&addr);
                            }
                        }
                    }
                }

                tracing::info!("Runner finished");
            }
            .instrument(tracing::info_span!("mDNS-direct")),
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

//
// This maps instance names to `PeerAddr`s and back.
//
// Example:
//   names:
//     "i1._ouisync._udp._local."     -> { 192.168.1.10:1234, 192.168.1.11:1234 }
//     "i1 (2)._ouisync._udp._local." -> { 192.168.1.10:1234, 192.168.1.11:1234 }
//     "i2._ouisync._udp._local."     -> { 192.168.1.20:1234 }
//
//   addrs:
//     192.168.1.10:1234 -> { "i1._ouisync._udp._local.", "i1 (2)._ouisync._udp._local" }
//     192.168.1.11:1234 -> { "i1._ouisync._udp._local.", "i1 (2)._ouisync._udp._local" }
//     192.168.1.20:1234 -> { "i2._ouisync._udp._local." }
//
struct Table {
    names: HashMap<String, HashSet<PeerAddr>>,
    addrs: HashMap<PeerAddr, HashSet<String>>,
}

impl Table {
    fn new() -> Self {
        Self {
            names: HashMap::new(),
            addrs: HashMap::new(),
        }
    }

    fn insert(&mut self, name: String, addr: PeerAddr) {
        self.names
            .entry(name.clone())
            .or_insert_with(HashSet::new)
            .insert(addr.clone());
        self.addrs
            .entry(addr)
            .or_insert_with(HashSet::new)
            .insert(name);
    }

    // Returns a set of addresses no longer referenced by any "name" after the removal.
    fn remove(&mut self, name: String) -> HashSet<PeerAddr> {
        let mut unreferenced_addrs = HashSet::new();

        let Some(removed_addrs) = self.names.remove(&name) else {
            return unreferenced_addrs;
        };

        for removed_addr in removed_addrs {
            if let Some(addr_names) = self.addrs.get_mut(&removed_addr) {
                addr_names.remove(&name);
                if addr_names.is_empty() {
                    self.addrs.remove(&removed_addr);
                    unreferenced_addrs.insert(removed_addr);
                }
            }
        }

        unreferenced_addrs
    }
}

fn generate_instance_name() -> String {
    // NOTE: The RFC 2763 section 4.1.1 says:
    //   https://datatracker.ietf.org/doc/html/rfc6763#section-4.1.1
    //
    //   The default name should be short and descriptive, and SHOULD NOT include the device's
    //   Media Access Control (MAC) address, serial number, or any similar incomprehensible
    //   hexadecimal string in an attempt to make the name globally unique.
    //
    // However, the suggestions for the names are to use _device_ types, which is not
    // what we can use.
    let bytes: [u8; 16] = rand::thread_rng().gen();
    format!("{}", hex::encode(&bytes))
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
