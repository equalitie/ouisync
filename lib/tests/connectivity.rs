//! Test for connectivity under various network configurations and conditions

#[macro_use]
mod common;

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use futures_util::future;
use ouisync::{AddrFilter, Network, PeerAddr, PeerState};
use patchbay::{Lab, RouterPreset};
use tokio::{sync::Barrier, time};

use common::{runtime_id_for, test_timeout};
use tracing::Instrument;

#[tokio::test]
async fn public_to_public() {
    setup();

    let lab = Lab::new().await.unwrap();
    let default_port = 10001;

    let a_router = lab
        .add_router("a.router")
        .preset(RouterPreset::Public)
        .build()
        .await
        .unwrap();
    let a_device = lab
        .add_device("a")
        .iface("eth0", a_router.id())
        .build()
        .await
        .unwrap();

    let b_router = lab
        .add_router("b.router")
        .preset(RouterPreset::Public)
        .build()
        .await
        .unwrap();
    let b_device = lab
        .add_device("b")
        .iface("eth0", b_router.id())
        .build()
        .await
        .unwrap();

    let a_addr = SocketAddr::from((a_device.ip().unwrap(), default_port));
    let b_addr = SocketAddr::from((b_device.ip().unwrap(), default_port));

    let a_barrier = Arc::new(Barrier::new(2));
    let b_barrier = a_barrier.clone();

    let a = a_device
        .spawn(async move |device| {
            async {
                let network = Network::builder()
                    .runtime_id(runtime_id_for(device.name()))
                    .addr_filter(AddrFilter::default().allow_benchmarking_v4())
                    .build();
                network
                    .bind(&[PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, default_port).into())])
                    .await;
                network.add_user_provided_peer(&PeerAddr::Quic(b_addr));

                expect_active(&network, "b").await;
                a_barrier.wait().await;
            }
            .instrument(info_span!("node", message = device.name()))
            .await
        })
        .unwrap();

    let b = b_device
        .spawn(async move |device| {
            async {
                let network = Network::builder()
                    .runtime_id(runtime_id_for(device.name()))
                    .build();
                network
                    .bind(&[PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, default_port).into())])
                    .await;

                expect_active(&network, "a").await;
                b_barrier.wait().await;
            }
            .instrument(info_span!("node", message = device.name()))
            .await
        })
        .unwrap();

    future::try_join(a, b).await.unwrap();
}

fn setup() {
    common::init_log();
}

async fn expect_active(network: &Network, peer_name: &str) {
    let peer_id = runtime_id_for(peer_name).public();
    let peer_info_collector = network.peer_info_collector();

    time::timeout(test_timeout(), async move {
        let mut rx = network.subscribe();

        loop {
            if peer_info_collector.collect().into_iter().any(|peer_info| {
                matches!(peer_info.state,
                    PeerState::Active { id, .. } if id == peer_id)
            }) {
                break;
            }

            rx.recv().await.unwrap();
        }
    })
    .await
    .unwrap()
}

// Initialize user namespace. This is needed for patchbay.
#[cfg(test)]
#[ctor::ctor(unsafe)]
fn init() {
    unsafe {
        patchbay::init_userns_for_ctor();
    }
}
