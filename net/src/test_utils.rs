use crate::{
    quic, tcp,
    unified::{Acceptor, Connection, Connector},
    SocketOptions,
};
use futures_util::future;
use std::net::Ipv4Addr;
use test_strategy::Arbitrary;

#[derive(Clone, Copy, Debug, Arbitrary)]
pub(crate) enum Proto {
    Tcp,
    Quic,
}

pub(crate) fn create_connected_peers(proto: Proto) -> (Connector, Acceptor) {
    let addr = (Ipv4Addr::LOCALHOST, 0).into();
    let options = SocketOptions::default();

    match proto {
        Proto::Tcp => {
            let (client, _) = tcp::configure(addr, options).unwrap();
            let (_, server) = tcp::configure(addr, options).unwrap();

            (client.into(), server.into())
        }
        Proto::Quic => {
            let (client, _, _) = quic::configure(addr, options).unwrap();
            let (_, server, _) = quic::configure(addr, options).unwrap();

            (client.into(), server.into())
        }
    }
}

pub(crate) async fn create_connected_connections(
    client: &Connector,
    server: &Acceptor,
) -> (Connection, Connection) {
    future::try_join(client.connect(*server.local_addr()), async {
        server.accept().await?.await
    })
    .await
    .unwrap()
}

pub(crate) fn init_log() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .compact()
        .try_init()
        .ok();
}
