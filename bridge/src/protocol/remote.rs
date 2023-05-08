use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Create repository on a remote server
    Create {
        // TODO: change type to ShareToken
        share_token: String,
    },
}

#[derive(Serialize, Deserialize)]
pub enum Response {
    None,
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}
