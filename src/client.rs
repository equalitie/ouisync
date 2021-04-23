use crate::message::Request;
use crate::message_broker::ClientStream;
use tokio::time::{sleep, Duration};

pub struct Client {}

impl Client {
    pub async fn run(&mut self, mut con: ClientStream) {
        println!("Client started");
        loop {
            con.write(Request::Hello).await;
            let rs = con.read().await;
            println!("Client received response: {:?}", rs);
            sleep(Duration::from_millis(1000)).await;
        }
    }
}
