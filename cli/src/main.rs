mod client;
mod handler;
mod options;
mod protocol;
mod repository;
mod server;
mod state;
mod transport;
mod utils;

use anyhow::Result;
use clap::Parser;
use options::Options;
use protocol::Request;

pub(crate) const APP_NAME: &str = "ouisync";
pub(crate) const DB_EXTENSION: &str = "ouisyncdb";

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if let Request::Start { log_format } = &options.request {
        server::run(options.dirs, options.socket, *log_format).await
    } else {
        client::run(options.dirs, options.socket, options.request).await
    }
}
