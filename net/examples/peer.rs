use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ouisync_net::{
    quic,
    tcp::{TcpListener, TcpStream},
};
use std::{
    future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    task, time,
};

const DEFAULT_PORT: u16 = 24816;
const DEFAULT_BIND_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
const DEFAULT_CONNECT_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const SEND_DELAY: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    match options.role {
        Role::Client => run_client(&options).await,
        Role::Server => run_server(&options).await,
    }
}

#[derive(Parser, Debug)]
struct Options {
    /// Client or Server
    #[arg(short, long)]
    role: Role,

    /// Protocol to use
    #[arg(short, long)]
    proto: Proto,

    /// If server, the address to bind to [default: 0.0.0.0]. If client the address to connect to
    /// [default: 127.0.0.1].
    #[arg(short, long)]
    addr: Option<IpAddr>,

    /// If server, the port to listen on. If client the port to connect to.
    #[arg(short = 'P', long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// If client, the number of messages to send (default is inifnity). If server, ignored.
    #[arg(short, long)]
    count: Option<usize>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Role {
    Client,
    Server,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Proto {
    Tcp,
    Quic,
}

async fn run_client(options: &Options) -> Result<()> {
    let addr: SocketAddr = (options.addr.unwrap_or(DEFAULT_CONNECT_ADDR), options.port).into();

    match options.proto {
        Proto::Tcp => run_tcp_client(addr, options.count).await,
        Proto::Quic => run_quic_client(addr, options.count).await,
    }
}

async fn run_tcp_client(addr: SocketAddr, count: Option<usize>) -> Result<()> {
    let stream = TcpStream::connect(addr).await?;
    run_client_connection(stream, count).await
}

async fn run_quic_client(addr: SocketAddr, count: Option<usize>) -> Result<()> {
    let (connector, _, _) = quic::configure((Ipv4Addr::UNSPECIFIED, 0).into()).await?;
    let connection = connector.connect(addr).await?;
    run_client_connection(connection, count).await
}

async fn run_server(options: &Options) -> Result<()> {
    let bind_addr: SocketAddr = (options.addr.unwrap_or(DEFAULT_BIND_ADDR), options.port).into();

    match options.proto {
        Proto::Tcp => run_tcp_server(bind_addr).await,
        Proto::Quic => run_quic_server(bind_addr).await,
    }
}

async fn run_tcp_server(addr: SocketAddr) -> Result<()> {
    let acceptor = TcpListener::bind(addr).await?;
    println!("bound to {}", acceptor.local_addr()?);

    loop {
        let (stream, addr) = acceptor.accept().await?;
        task::spawn(run_server_connection(stream, addr));
    }
}

async fn run_quic_server(addr: SocketAddr) -> Result<()> {
    let (_, mut acceptor, _) = quic::configure(addr).await?;
    println!("bound to {}", acceptor.local_addr());

    loop {
        let connection = acceptor
            .accept()
            .await
            .context("failed to accept")?
            .finish()
            .await?;
        let addr = *connection.remote_address();
        task::spawn(run_server_connection(connection, addr));
    }
}

async fn run_client_connection<T: AsyncRead + AsyncWrite + Unpin>(
    mut stream: T,
    count: Option<usize>,
) -> Result<()> {
    println!("connected");

    let message = "hello world";
    let mut i = 0;

    loop {
        if count.map(|count| i >= count).unwrap_or(false) {
            break;
        }

        i = i.saturating_add(1);

        println!("sending  \"{message}\"");
        write_message(&mut stream, message).await?;

        let response = read_message(&mut stream).await?;
        println!("received \"{response}\"");

        time::sleep(SEND_DELAY).await;
    }

    future::pending().await
}

async fn run_server_connection<T: AsyncRead + AsyncWrite + Unpin>(mut stream: T, addr: SocketAddr) {
    println!("[{}] accepted", addr);

    loop {
        let message = match read_message(&mut stream).await {
            Ok(message) => message,
            Err(error) => {
                println!("[{}] read failed: {}", addr, error);
                break;
            }
        };

        println!("[{}] received \"{}\"", addr, message);

        match write_message(&mut stream, "ok").await {
            Ok(_) => (),
            Err(error) => {
                println!("[{}] write failed: {}", addr, error);
                break;
            }
        }
    }

    println!("[{}] closed", addr);
}

async fn read_message<T: AsyncRead + Unpin>(reader: &mut T) -> Result<String> {
    let size = reader.read_u32().await? as usize;
    let mut buffer = vec![0; size];
    reader.read_exact(&mut buffer).await?;
    Ok(String::from_utf8(buffer)?)
}

async fn write_message<T: AsyncWrite + Unpin>(writer: &mut T, message: &str) -> Result<()> {
    writer.write_u32(message.len() as u32).await?;
    writer.write_all(message.as_bytes()).await?;
    Ok(())
}
