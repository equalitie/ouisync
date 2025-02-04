use crate::network::{
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use rand::Rng;
use std::collections::{HashMap, HashSet};

pub(crate) fn generate_instance_name() -> String {
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
    hex::encode(bytes)
}

pub(crate) struct SeenMdnsPeers {
    seen_peers: SeenPeers,
    table: Table,
}

impl SeenMdnsPeers {
    pub(crate) fn new() -> Self {
        Self {
            seen_peers: SeenPeers::new(),
            table: Table::new(),
        }
    }

    pub(crate) fn insert(&mut self, name: String, address: PeerAddr) -> Option<SeenPeer> {
        self.table.insert(name, address);
        self.seen_peers.insert(address)
    }

    pub(crate) fn remove(&mut self, name: String) {
        for addr in self.table.remove(name) {
            self.seen_peers.remove(&addr);
        }
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
pub(crate) struct Table {
    names: HashMap<String, HashSet<PeerAddr>>,
    addrs: HashMap<PeerAddr, HashSet<String>>,
}

impl Table {
    pub(crate) fn new() -> Self {
        Self {
            names: HashMap::new(),
            addrs: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, name: String, addr: PeerAddr) {
        self.names.entry(name.clone()).or_default().insert(addr);
        self.addrs.entry(addr).or_default().insert(name);
    }

    // Returns a set of addresses no longer referenced by any "name" after the removal.
    pub fn remove(&mut self, name: String) -> HashSet<PeerAddr> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_test() {
        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let foo_addr = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());

            table.insert(foo_name.clone(), foo_addr);

            assert_eq!(table.remove(foo_name), [foo_addr].into());
        }
        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let foo_addr1 = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());
            let foo_addr2 = PeerAddr::Quic(([192, 168, 1, 3], 1000).into());

            table.insert(foo_name.clone(), foo_addr1);
            table.insert(foo_name.clone(), foo_addr2);

            assert_eq!(table.remove(foo_name), [foo_addr1, foo_addr2].into());
        }

        {
            let mut table = Table::new();

            let foo_name = "foo".to_owned();
            let bar_name = "bar".to_owned();
            let addr = PeerAddr::Quic(([192, 168, 1, 2], 1000).into());

            table.insert(foo_name.clone(), addr);
            table.insert(bar_name.clone(), addr);

            assert_eq!(table.remove(foo_name), [].into());
            assert_eq!(table.remove(bar_name), [addr].into());
        }
    }
}
