use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Hello,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    Hello,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Request(Request),
    Response(Response),
}

impl Message {
    pub fn into_response(self) -> Response {
        match self {
            Message::Request(_) => panic!("Message is not a response"),
            Message::Response(rs) => rs,
        }
    }

    pub fn into_request(self) -> Request {
        match self {
            Message::Request(rq) => rq,
            Message::Response(_) => panic!("Message is not a request"),
        }
    }
}
