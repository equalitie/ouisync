use crate::replica_discovery::ReplicaDiscovery;
use crate::replica_id::ReplicaId;

use std::{io, sync::Arc, net::SocketAddr, collections::HashSet};
use futures::future::{abortable, AbortHandle};

use tokio::{
    task::spawn,
    net::{TcpStream, TcpListener},
    io::AsyncWriteExt,
};

pub struct Network {
    abort_handles: HashSet<AbortHandle>,
    this_replica_id: ReplicaId,
}

impl Network {
    pub fn new(_enable_discovery: bool) -> io::Result<Arc<Network>> {
        let n = Arc::new(Network{
            abort_handles: HashSet::new(),
            this_replica_id: ReplicaId::random(),
        });
        let n_ = n.clone();
        spawn(async move { n_.start().await.unwrap(); });
        Ok(n)
    }

    async fn start(self: Arc<Self>) -> io::Result<()> {
        let any_addr = SocketAddr::from(([0,0,0,0], 0));
        let listener = TcpListener::bind(any_addr).await?;
        
        let s1 = self.clone();
        let s2 = self.clone();

        spawn(s1.run_discovery(listener.local_addr().unwrap().port()));
        spawn(s2.run_listener(listener));

        Ok(())
    }

    async fn run_discovery(self: Arc<Self>, listener_port: u16) -> io::Result<()> {
        let mut discovery = ReplicaDiscovery::new(listener_port)?;

        loop {
            let found = discovery.wait_for_activity().await;

            for addr in found {
                let s = self.clone();
                spawn(async move {
                    let socket = match TcpStream::connect(addr).await {
                        Ok(socket) => socket,
                        Err(_) => return,
                    };
                    s.handle_new_connection(socket).await;
                });
            }
        }
    }

    async fn run_listener(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            let (socket, addr) = listener.accept().await?;
            let s = self.clone();
            spawn(async move {
                s.handle_new_connection(socket).await;
            });
        }
    }

    async fn handle_new_connection(self: Arc<Self>, mut socket: TcpStream) -> io::Result<()> {
        println!("Got new connection");
        Ok(())
    }
}
