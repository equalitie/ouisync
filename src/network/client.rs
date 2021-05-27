use super::{message::Request, message_broker::ClientStream};
use crate::Index;
use tokio::time::{sleep, Duration};

pub struct Client {}

impl Client {
    pub async fn run(&mut self, mut con: ClientStream, _index: &Index) {
        println!("Client started");
        loop {
            if con.write(Request::Hello).await.is_err() {
                return;
            }
            let rs = con.read().await;
            println!("Client received response: {:?}", rs);
            sleep(Duration::from_millis(1000)).await;
        }
    }
}
