use anyhow::Result;
use clap::{Parser, ValueEnum};
use ouisync_net::{
    connection::{Acceptor, Connection, Connector, RecvStream, SendStream},
    quic, tcp,
};
use std::{
    future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
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
    let connector = match options.proto {
        Proto::Tcp => {
            let (connector, _) = tcp::configure((DEFAULT_BIND_ADDR, 0).into())?;
            Connector::Tcp(connector)
        }
        Proto::Quic => {
            let (connector, _, _) = quic::configure((DEFAULT_BIND_ADDR, 0).into())?;
            Connector::Quic(connector)
        }
    };

    let connection = connector.connect(addr).await?;
    let (mut tx, mut rx) = connection.outgoing().await?;

    println!("connected");

    let message = "hello world";
    let mut i = 0;

    loop {
        if options.count.map(|count| i >= count).unwrap_or(false) {
            break;
        }

        i = i.saturating_add(1);

        println!("sending  \"{message}\"");
        write_message(&mut tx, message).await?;

        let response = read_message(&mut rx).await?;
        println!("received \"{response}\"");

        time::sleep(SEND_DELAY).await;
    }

    future::pending().await
}

async fn run_server(options: &Options) -> Result<()> {
    let bind_addr: SocketAddr = (options.addr.unwrap_or(DEFAULT_BIND_ADDR), options.port).into();

    let acceptor = match options.proto {
        Proto::Tcp => {
            let (_, acceptor) = tcp::configure(bind_addr)?;
            Acceptor::Tcp(acceptor)
        }
        Proto::Quic => {
            let (_, acceptor, _) = quic::configure(bind_addr)?;
            Acceptor::Quic(acceptor)
        }
    };

    println!("bound to {}", acceptor.local_addr());

    loop {
        let connection = acceptor.accept().await?.await?;
        task::spawn(run_server_connection(connection));
    }
}

async fn run_server_connection(connection: Connection) {
    let addr = connection.remote_addr();

    println!("[{}] accepted", addr);

    let (mut tx, mut rx) = match connection.incoming().await {
        Ok(stream) => stream,
        Err(error) => {
            println!("[{}] accept stream failed: {}", addr, error);
            return;
        }
    };

    loop {
        let message = match read_message(&mut rx).await {
            Ok(message) => message,
            Err(error) => {
                println!("[{}] read failed: {}", addr, error);
                break;
            }
        };

        println!("[{}] received \"{}\"", addr, message);

        match write_message(&mut tx, "ok").await {
            Ok(_) => (),
            Err(error) => {
                println!("[{}] write failed: {}", addr, error);
                break;
            }
        }
    }

    println!("[{}] closed", addr);
}

async fn read_message(reader: &mut RecvStream) -> Result<String> {
    let size = reader.read_u32().await? as usize;
    let mut buffer = vec![0; size];
    reader.read_exact(&mut buffer).await?;
    Ok(String::from_utf8(buffer)?)
}

async fn write_message(writer: &mut SendStream, message: &str) -> Result<()> {
    writer.write_u32(message.len() as u32).await?;
    writer.write_all(message.as_bytes()).await?;
    Ok(())
}
