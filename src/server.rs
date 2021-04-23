use crate::message::Response;
use crate::message_broker::ServerStream;

pub struct Server {}

impl Server {
    pub async fn run(&mut self, mut con: ServerStream) {
        println!("Server started");
        loop {
            let rq = con.read().await;
            println!("Server: received request {:?}", rq);
            con.write(Response::Hello).await;
        }
    }
}
