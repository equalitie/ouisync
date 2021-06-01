use super::{message::Request, message_broker::ClientStream};
use crate::Index;
use tokio::time::{sleep, Duration};

pub struct Client {}

impl Client {
    pub async fn run(&mut self, mut con: ClientStream, _index: &Index) {
        log::debug!("Client started");

        loop {
            if con.send(Request::Hello).await.is_err() {
                return;
            }
            let rs = con.recv().await;
            log::debug!("Client received response: {:?}", rs);
            sleep(Duration::from_secs(10)).await;
        }
    }
}
