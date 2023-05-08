use ouisync_lib::ShareToken;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Mirror repository on a remote server
    Mirror { share_token: ShareToken },
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
