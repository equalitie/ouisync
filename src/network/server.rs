use super::{message::Response, message_broker::ServerStream};
use crate::Index;

pub struct Server {}

impl Server {
    pub async fn run(&mut self, mut con: ServerStream, _index: &Index) {
        log::debug!("Server started");

        loop {
            let rq = con.recv().await;
            log::debug!("Server: received request {:?}", rq);
            if con.send(Response::Hello).await.is_err() {
                return;
            }
        }
    }
}
