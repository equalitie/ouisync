use std::{collections::HashSet, future, io, mem, net::SocketAddr, time::Duration};

use clap::Parser;
use ouisync_service::Service;
use rand::{Rng, SeedableRng, rngs::StdRng};
use tokio::{runtime, select, time};
#[path = "../src/test_utils.rs"]
mod test_utils;

const MIN_PACKET_SIZE: usize = 32;
const MAX_PACKET_SIZE: usize = 512;
const MAX_PENDING_PACKETS: usize = 64;

#[derive(clap::Parser, Debug)]
struct Options {
    /// Transport used by the client
    #[arg(short, long)]
    client: Transport,

    /// Transport used by the server
    #[arg(short, long)]
    server: Transport,

    /// Number of packets to send
    #[arg(long, default_value_t = 1024)]
    count: usize,

    /// Random number generator seed for repeatable runs
    #[arg(long)]
    seed: Option<u64>,

    // The following arguments may be passed down from `cargo bench` so we need to accept them even
    // if we don't use them.
    #[arg(
        long = "bench",
        hide = true,
        hide_short_help = true,
        hide_long_help = true
    )]
    _bench: bool,

    #[arg(
        long = "profile-time",
        hide = true,
        hide_short_help = true,
        hide_long_help = true
    )]
    _profile_time: Option<String>,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Transport {
    /// Use raw UDP sockets
    Raw,
    /// Use ouisync
    Oui,
}

fn main() {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run());
}

async fn run() {
    test_utils::init_log();

    let options = Options::parse();
    let rng = if let Some(seed) = options.seed {
        StdRng::seed_from_u64(seed)
    } else {
        StdRng::from_entropy()
    };

    match (options.client, options.server) {
        (Transport::Oui, Transport::Oui) => {
            case::<oui::Socket, oui::Socket>(rng, options.count).await
        }
        (Transport::Oui, Transport::Raw) => {
            case::<oui::Socket, raw::Socket>(rng, options.count).await
        }
        (Transport::Raw, Transport::Raw) => {
            case::<raw::Socket, raw::Socket>(rng, options.count).await
        }
        (Transport::Raw, Transport::Oui) => {
            case::<raw::Socket, oui::Socket>(rng, options.count).await
        }
    }
}

const HEADER_LEN: usize = mem::size_of::<usize>();

async fn case<Client: Socket, Server: Socket>(rng: StdRng, packet_count: usize) {
    let (mut client_reader, mut client_writer, client_addr) = Client::create().await;
    let (mut server_reader, mut server_writer, server_addr) = Server::create().await;

    async fn client(
        reader: &mut impl Reader,
        writer: &mut impl Writer,
        peer_addr: SocketAddr,
        packet_count: usize,
        mut rng: StdRng,
    ) {
        let packets: Vec<_> = (0..packet_count)
            .map(|_| {
                let size = rng.gen_range(MIN_PACKET_SIZE..=MAX_PACKET_SIZE);

                let mut data = vec![0u8; size];
                rng.fill(&mut data[..]);

                data
            })
            .collect();

        let mut pending = HashSet::with_capacity(packets.len());

        let mut sent = 0;
        let mut ackd = 0;

        let mut send_buf = vec![0; 1024];
        let mut recv_buf = vec![0; 1024];

        async fn send(
            writer: &mut impl Writer,
            packets: &[Vec<u8>],
            index: usize,
            peer_addr: SocketAddr,
            buf: &mut Vec<u8>,
            pending: usize,
        ) {
            if index >= packets.len() || pending >= MAX_PENDING_PACKETS {
                return future::pending().await;
            }

            let data = &packets[index];

            buf.resize(HEADER_LEN + data.len(), 0);
            buf[..HEADER_LEN].copy_from_slice(&index.to_ne_bytes());
            buf[HEADER_LEN..].copy_from_slice(&data[..]);

            writer.send_to(buf, peer_addr).await.unwrap();
        }

        async fn recv(
            reader: &mut impl Reader,
            peer_addr: SocketAddr,
            buf: &mut [u8],
            pending: usize,
        ) -> usize {
            if pending == 0 {
                return future::pending().await;
            }

            let (size, addr) = reader.recv_from(buf).await.unwrap();
            assert!(size >= HEADER_LEN);
            assert_eq!(addr, peer_addr);

            usize::from_ne_bytes(buf[..HEADER_LEN].try_into().unwrap())
        }

        while sent < packets.len() || !pending.is_empty() {
            select! {
                _ = send(writer, &packets, sent, peer_addr, &mut send_buf, pending.len()) => {
                    pending.insert(sent);
                    sent += 1;
                }
                id = recv(reader, peer_addr, &mut recv_buf, pending.len()) => {
                    if pending.remove(&id) {
                        ackd += 1;
                    }
                }
                _ = time::sleep(Duration::from_secs(1)) => panic!("timeout"),
            }

            tracing::debug!("sent: {}, ackd: {}", sent, ackd);
        }
    }

    async fn server(reader: &mut impl Reader, writer: &mut impl Writer, peer_addr: SocketAddr) {
        let mut buf = vec![0; 1024];

        loop {
            buf.resize(1024, 0);
            let (size, addr) = reader.recv_from(&mut buf).await.unwrap();
            buf.truncate(size);

            assert_eq!(addr, peer_addr);

            let id = usize::from_ne_bytes(buf[..HEADER_LEN].try_into().unwrap());
            writer.send_to(&id.to_ne_bytes(), addr).await.unwrap();
        }
    }

    select! {
        _ = client(&mut client_reader, &mut client_writer, server_addr, packet_count, rng) => (),
        _ = server(&mut server_reader, &mut server_writer, client_addr) => (),
    }

    client_reader.close().await;
    client_writer.close().await;

    server_reader.close().await;
    server_writer.close().await;
}

trait Socket {
    type Reader: Reader;
    type Writer: Writer;

    async fn create() -> (Self::Reader, Self::Writer, SocketAddr);
}

trait Reader {
    async fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, SocketAddr), io::Error>;
    async fn close(self);
}

trait Writer {
    async fn send_to(&mut self, buf: &[u8], addr: SocketAddr) -> Result<usize, io::Error>;
    async fn close(self);
}

mod oui {
    use std::{
        io,
        net::{Ipv4Addr, SocketAddr},
        sync::Arc,
    };

    use ouisync::PeerAddr;
    use ouisync_service::{
        Service, local_endpoint,
        protocol::{Datagram, NetworkSocketHandle, Request},
        transport::local::LocalClient,
    };
    use tempfile::TempDir;

    use crate::test_utils::ServiceRunner;

    struct Shared {
        service_runner: ServiceRunner,
        _temp_dir: TempDir,
    }

    pub(super) struct Reader {
        shared: Arc<Shared>,
        client: LocalClient,
        socket_handle: NetworkSocketHandle,
    }

    impl super::Reader for Reader {
        async fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, SocketAddr), io::Error> {
            let datagram: Datagram = self
                .client
                .invoke(Request::NetworkSocketRecvFrom {
                    socket: self.socket_handle,
                    len: buf.len() as u64,
                })
                .await
                .map_err(io::Error::other)?;

            let n = buf.len().min(datagram.data.len());
            buf[..n].copy_from_slice(&datagram.data[..n]);

            Ok((n, datagram.addr))
        }

        async fn close(self) {
            close(self.shared, self.client).await;
        }
    }

    pub(super) struct Writer {
        shared: Arc<Shared>,
        client: LocalClient,
        socket_handle: NetworkSocketHandle,
    }

    impl super::Writer for Writer {
        async fn send_to(&mut self, buf: &[u8], addr: SocketAddr) -> Result<usize, io::Error> {
            let n: u64 = self
                .client
                .invoke(Request::NetworkSocketSendTo {
                    socket: self.socket_handle,
                    data: buf.to_vec().into(),
                    addr,
                })
                .await
                .map_err(io::Error::other)?;

            Ok(n as usize)
        }

        async fn close(self) {
            close(self.shared, self.client).await;
        }
    }

    pub(super) struct Socket;

    impl super::Socket for Socket {
        type Reader = Reader;
        type Writer = Writer;

        async fn create() -> (Reader, Writer, SocketAddr) {
            let temp_dir = TempDir::new().unwrap();
            let service = Service::init(temp_dir.path().to_owned()).await.unwrap();
            let service_runner = ServiceRunner::start(service);

            let endpoint = local_endpoint(temp_dir.path()).await.unwrap();
            let mut client = LocalClient::connect(endpoint).await.unwrap();

            let _: () = client
                .invoke(Request::SessionBindNetwork {
                    addrs: vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())],
                })
                .await
                .unwrap();

            let socket_handle: NetworkSocketHandle = client
                .invoke(Request::SessionOpenNetworkSocketV4)
                .await
                .unwrap();

            let addrs: Vec<PeerAddr> = client
                .invoke(Request::SessionGetLocalListenerAddrs)
                .await
                .unwrap();
            let addr = match addrs.into_iter().next().unwrap() {
                PeerAddr::Quic(addr) => addr,
                PeerAddr::Tcp(_) => unreachable!(),
            };

            let shared = Shared {
                service_runner,
                _temp_dir: temp_dir,
            };
            let shared = Arc::new(shared);

            (
                Reader {
                    shared: shared.clone(),
                    client,
                    socket_handle,
                },
                Writer {
                    shared,
                    client: LocalClient::connect(endpoint).await.unwrap(),
                    socket_handle,
                },
                addr,
            )
        }
    }

    async fn close(shared: Arc<Shared>, mut client: LocalClient) {
        client.close().await.unwrap();

        if let Some(shared) = Arc::into_inner(shared) {
            shared.service_runner.stop().await.close().await;
        }
    }
}

mod raw {
    use std::{
        io,
        net::{Ipv4Addr, SocketAddr},
        sync::Arc,
    };

    use tokio::net::UdpSocket;

    pub(super) struct Reader(Arc<UdpSocket>);

    impl super::Reader for Reader {
        async fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, SocketAddr), io::Error> {
            self.0.recv_from(buf).await
        }

        async fn close(self) {}
    }

    pub(super) struct Writer(Arc<UdpSocket>);

    impl super::Writer for Writer {
        async fn send_to(&mut self, buf: &[u8], addr: SocketAddr) -> Result<usize, io::Error> {
            self.0.send_to(buf, addr).await
        }

        async fn close(self) {}
    }

    pub(super) struct Socket;

    impl super::Socket for Socket {
        type Reader = Reader;
        type Writer = Writer;

        async fn create() -> (Reader, Writer, SocketAddr) {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let socket = Arc::new(socket);

            let addr = socket.local_addr().unwrap();

            (Reader(socket.clone()), Writer(socket), addr)
        }
    }
}
