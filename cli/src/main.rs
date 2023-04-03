mod client;
mod handler;
mod host_addr;
mod options;
mod repository;
mod server;
mod transport;
mod utils;

use self::options::{Options, Request};
use anyhow::Result;
use clap::Parser;

pub(crate) const APP_NAME: &str = "ouisync";
pub(crate) const DB_EXTENSION: &str = "ouisyncdb";

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if let Request::Serve = options.request {
        server::run(options).await
    } else {
        client::run(options).await
    }
}
