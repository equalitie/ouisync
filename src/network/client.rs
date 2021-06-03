use super::message_broker::ClientStream;
use crate::Index;

pub struct Client {}

impl Client {
    pub async fn run(&mut self, mut _stream: ClientStream, _index: &Index) {
        log::debug!("Client started");
        todo!()
    }
}
