use ouisync::Error;
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use structopt::StructOpt;

#[derive(StructOpt)]
struct Options {
    /// Base directory
    #[structopt(short, long)]
    base_dir: PathBuf,

    /// Mount directory
    #[structopt(short, long)]
    mount_dir: Option<PathBuf>,

    /// Peer's endpoint
    #[structopt(short, long, value_name = "ip:port")]
    connect: Option<SocketAddr>,

    /// Port to listen on
    #[structopt(short, long, default_value = "65535")]
    port: u16,

    /// IP address to bind to
    #[structopt(long, default_value = "0.0.0.0")]
    bind: IpAddr,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let options = Options::from_args();

    println!(
        "base_dir: {}, mount_dir: {:?}, connect: {:?}, bind: {}",
        options.base_dir.display(),
        options.mount_dir,
        options.connect,
        SocketAddr::from((options.bind, options.port))
    );

    ouisync::sql_example().await?;
    ouisync::hello_world()?;

    Ok(())
}
