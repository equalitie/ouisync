mod client;
mod handler;
mod host_addr;
mod options;
mod server;
mod transport;

use self::options::{Options, Request};
use anyhow::Result;
use clap::Parser;

pub(crate) const APP_NAME: &str = "ouisync";

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    if let Request::Serve = options.request {
        server::run(options).await
    } else {
        client::run(options).await
    }
}
