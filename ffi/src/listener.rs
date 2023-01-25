use super::session::State;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinSet,
};

pub(crate) enum ListenerStatus {
    Starting,
    Running(SocketAddr),
    Failed(io::Error),
}

pub(crate) async fn run(state: Arc<State>, status_tx: watch::Sender<ListenerStatus>) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => {
            status_tx.send(ListenerStatus::Failed(error)).ok();
            return;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => {
            status_tx.send(ListenerStatus::Running(addr)).ok();
            addr
        }
        Err(error) => {
            status_tx.send(ListenerStatus::Failed(error)).ok();
            return;
        }
    };

    tracing::debug!(?local_addr, "interface listener started");

    let mut clients = JoinSet::new();

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                clients.spawn(client(stream, addr, state.clone()));
            }
            Err(error) => {
                status_tx.send(ListenerStatus::Failed(error)).ok();
                break;
            }
        }
    }
}

async fn client(_stream: TcpStream, _addr: SocketAddr, _state: Arc<State>) {
    todo!()
}
