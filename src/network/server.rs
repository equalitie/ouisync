use super::{message::Response, message_broker::ServerStream};
use crate::Index;

pub struct Server {}

impl Server {
    pub async fn run(&mut self, mut con: ServerStream, _index: &Index) {
        println!("Server started");
        loop {
            let rq = con.read().await;
            println!("Server: received request {:?}", rq);
            if con.write(Response::Hello).await.is_err() {
                return;
            }
        }
    }
}
