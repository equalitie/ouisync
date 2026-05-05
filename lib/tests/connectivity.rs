//! Test for connectivity under various network configurations and conditions

#[macro_use]
mod common;

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::pin,
    sync::Arc,
};

use btdht::{INFO_HASH_LEN, InfoHash};
use futures_util::{StreamExt, future};
use ouisync::{AddrFilter, Network, PeerAddr, PeerState, PublicRuntimeId, SecretRuntimeId};
use patchbay::{Device, Lab, Nat, NatConfig, NatFiltering, NatMapping, Router, RouterPreset};
use tokio::{
    net::UdpSocket,
    select,
    sync::Barrier,
    task::{self, JoinHandle},
    time,
};

use common::{dht::TestDhtContacts, runtime_id_for, test_timeout};
use tracing::instrument;

#[tokio::test]
async fn lan() {
    let lab = setup().await;

    let router = make_router(&lab, "home", RouterPreset::Home).await;
    let device_0 = make_device(&lab, "alice", &router).await;
    let device_1 = make_device(&lab, "bob", &router).await;

    case(&lab, &[device_0, device_1], 0).await;
}

#[tokio::test]
async fn public_to_public() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::Public).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Public).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn home_to_public() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::Home).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Public).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn home_to_home() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::Home).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Home).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn cgnat_to_public() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::IspCgnat).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Public).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn cgnat_to_home() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::IspCgnat).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Home).await;

    case(&lab, &[device_0, device_1], 1).await;
}

// FIXME: this doesn't work, find out why (possibly something to do with hairpinning?)
#[ignore]
#[tokio::test]
async fn cgnat_to_same_cgnat() {
    let lab = setup().await;

    let router = lab
        .add_router("isp")
        .preset(RouterPreset::IspCgnat)
        .build()
        .await
        .unwrap();
    let device_0 = make_device(&lab, "alice", &router).await;
    let device_1 = make_device(&lab, "bob", &router).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn cgnat_to_other_cgnat() {
    let lab = setup().await;

    let (_router_0, device_0) = make_router_and_device(&lab, "alice", RouterPreset::IspCgnat).await;
    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::IspCgnat).await;

    case(&lab, &[device_0, device_1], 1).await;
}

#[tokio::test]
async fn endpoint_dependent_to_public() {
    let lab = setup().await;
    let router_0 = make_router_with_endpoint_dependent_nat(&lab, "alice.router").await;
    let device_0 = make_device(&lab, "alice", &router_0).await;

    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Public).await;

    case(&lab, &[device_0, device_1], 1).await;
}

// FIXME: this case currently doesn't work. The usual way to solve this is using relay, but another
// option could be the "birthday paradox" approach as described in
// https://tailscale.com/blog/how-nat-traversal-works.
#[ignore]
#[tokio::test]
async fn endpoint_dependent_to_home() {
    let lab = setup().await;
    let router_0 = make_router_with_endpoint_dependent_nat(&lab, "alice.router").await;
    let device_0 = make_device(&lab, "alice", &router_0).await;

    let (_router_1, device_1) = make_router_and_device(&lab, "bob", RouterPreset::Home).await;

    case(&lab, &[device_0, device_1], 1).await;
}

// -------------------------------------------------------------------------------------------------
// Utilities

async fn setup() -> Lab {
    common::init_log();
    Lab::new().await.unwrap()
}

async fn make_device(lab: &Lab, name: &str, router: &Router) -> Device {
    lab.add_device(name)
        .iface("eth0", router.id())
        .build()
        .await
        .unwrap()
}

async fn make_router(lab: &Lab, name: &str, preset: RouterPreset) -> Router {
    lab.add_router(name).preset(preset).build().await.unwrap()
}

async fn make_router_with_endpoint_dependent_nat(lab: &Lab, name: &str) -> Router {
    lab.add_router(name)
        .preset(RouterPreset::Home)
        .nat(Nat::Custom(
            NatConfig::builder()
                .mapping(NatMapping::EndpointDependent)
                .filtering(NatFiltering::AddressAndPortDependent)
                .build(),
        ))
        .build()
        .await
        .unwrap()
}

async fn make_router_and_device(lab: &Lab, name: &str, preset: RouterPreset) -> (Router, Device) {
    let router = make_router(lab, &format!("{name}.router"), preset).await;
    let device = make_device(lab, name, &router).await;

    (router, device)
}

async fn case(lab: &Lab, devices: &[Device], num_dht_nodes: usize) {
    let dht = TestDht::setup(lab, num_dht_nodes).await;
    let dht_contacts = Arc::new(TestDhtContacts::new([dht.bootstrap_addr]));

    let barrier = Arc::new(Barrier::new(devices.len()));
    let secret_runtime_ids: Vec<_> = devices
        .iter()
        .map(|device| runtime_id_for(device.name()))
        .collect();
    let public_runtime_ids: Vec<_> = secret_runtime_ids.iter().map(|id| id.public()).collect();

    let info_hash: [u8; INFO_HASH_LEN] = rand::random();
    let info_hash = InfoHash::from(info_hash);

    let handles: Vec<_> = devices
        .iter()
        .zip(secret_runtime_ids)
        .enumerate()
        .map(|(i, (device, id))| {
            let dht_contacts = dht_contacts.clone();
            let peer_ids: Vec<_> = public_runtime_ids
                .iter()
                .enumerate()
                .filter(|(j, _)| i != *j)
                .map(|(_, id)| *id)
                .collect();
            let barrier = barrier.clone();

            device
                .spawn(move |device| {
                    run_node(device, id, peer_ids, dht_contacts, info_hash, barrier)
                })
                .unwrap()
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

struct TestDht {
    bootstrap_addr: SocketAddr,
    #[expect(dead_code)]
    devices: Vec<Device>,
    handles: Vec<JoinHandle<()>>,
}

impl TestDht {
    async fn setup(lab: &Lab, count: usize) -> Self {
        if count == 0 {
            return Self {
                bootstrap_addr: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
                devices: Vec::new(),
                handles: Vec::new(),
            };
        }

        // All DHT nodes are exposed to the (simulated) internet, so we use a single "public" router
        // for all of them for simplicity.
        let router = lab
            .add_router("dht-router")
            .preset(RouterPreset::Public)
            .build()
            .await
            .unwrap();

        let devices = future::join_all((0..count).map(async |i| {
            lab.add_device(&format!("dht-node-{i}"))
                .iface("eth0", router.id())
                .build()
                .await
                .unwrap()
        }))
        .await;

        let bootstrap_ip = devices[0].ip().unwrap();
        let bootstrap_port = 50001;
        let bootstrap_addr = SocketAddr::from((bootstrap_ip, bootstrap_port));

        let handles: Vec<_> = devices
            .iter()
            .map(|device| {
                device
                    .spawn(move |device| run_dht_node(device, bootstrap_addr))
                    .unwrap()
            })
            .collect();

        Self {
            bootstrap_addr: (bootstrap_ip, bootstrap_port).into(),
            devices,
            handles,
        }
    }
}

impl Drop for TestDht {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }
}

#[instrument(name = "node", skip_all, fields(message = device.name()), target = "ouisync-test")]
async fn run_dht_node(device: Device, bootstrap_addr: SocketAddr) {
    let builder = btdht::MainlineDht::builder().set_read_only(false);

    let (builder, socket) = if device.ip().map(IpAddr::V4) == Some(bootstrap_addr.ip()) {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, bootstrap_addr.port()))
            .await
            .unwrap();
        (builder, socket)
    } else {
        let builder = builder.add_node(bootstrap_addr);
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
        (builder, socket)
    };

    let _dht = builder.start(socket);

    future::pending::<()>().await;
}

#[instrument(name = "node", skip_all, fields(message = device.name()), target = "ouisync-test")]
async fn run_node(
    device: Device,
    this_id: SecretRuntimeId,
    peer_ids: Vec<PublicRuntimeId>,
    dht_contacts: Arc<TestDhtContacts>,
    info_hash: InfoHash,
    barrier: Arc<Barrier>,
) {
    let network = Network::builder()
        .runtime_id(this_id)
        .dht_contacts(dht_contacts)
        .addr_filter(AddrFilter::default().allow_benchmarking_v4())
        .build();

    network.set_dht_routers(HashSet::new());
    network.set_local_discovery_enabled(true);
    network
        .bind(&[PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into())])
        .await;

    let _dht = network.pin_dht().await;

    select! {
        _ = find_peers_on_dht(&network, info_hash) => unreachable!(),
        _ = wait_connected(&network, peer_ids) => (),
    }

    barrier.wait().await;
}

async fn find_peers_on_dht(network: &Network, info_hash: InfoHash) {
    loop {
        let mut stream = pin!(network.dht_lookup(info_hash, true));
        while let Some(addr) = stream.next().await {
            network.add_user_provided_peer(&addr);
        }

        task::yield_now().await;
    }
}

async fn wait_connected(network: &Network, expected_peers: Vec<PublicRuntimeId>) {
    let peer_info_collector = network.peer_info_collector();

    time::timeout(test_timeout(), async move {
        let mut rx = network.subscribe();

        loop {
            let actual_peers: HashSet<_> = peer_info_collector
                .collect()
                .into_iter()
                .filter_map(|info| match info.state {
                    PeerState::Active { id, .. } => Some(id),
                    _ => None,
                })
                .collect();

            if expected_peers.iter().all(|id| actual_peers.contains(id)) {
                break;
            }

            rx.recv().await.unwrap();
        }
    })
    .await
    .unwrap()
}

// Initialize user namespace. This needs to run before anything else (especially before any threads
// are spawned). Using the `ctor` crate to achieve that.
#[cfg(test)]
#[ctor::ctor(unsafe)]
fn init() {
    unsafe {
        patchbay::init_userns_for_ctor();
    }
}
