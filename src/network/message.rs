use crate::crypto::Hash;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    /// Request the latest root node from another replica.
    RootNode,
    /// Request inner nodes with the given parent hash.
    InnerNodes(Hash),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    /// Send the latest root node of this replica to another replica.
    RootNode(Hash),
    /// Send all inner nodes with the given parent hash.
    InnerNodes {
        parent_hash: Hash,
        children: Vec<Hash>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Request(Request),
    Response(Response),
}

impl From<Message> for Request {
    fn from(msg: Message) -> Self {
        match msg {
            Message::Request(rq) => rq,
            Message::Response(_) => panic!("Message is not Request"),
        }
    }
}

impl From<Message> for Response {
    fn from(msg: Message) -> Self {
        match msg {
            Message::Request(_) => panic!("Message is not Response"),
            Message::Response(rs) => rs,
        }
    }
}
