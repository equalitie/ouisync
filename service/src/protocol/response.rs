use crate::repository::RepositoryHandle;
use ouisync::{PeerAddr, PeerInfo, StorageSize};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    None,
    Bool(bool),
    Expiration {
        block: Option<Duration>,
        repository: Option<Duration>,
    },
    Path(PathBuf),
    PeerAddrs(Vec<PeerAddr>),
    PeerInfo(Vec<PeerInfo>),
    QuotaInfo(QuotaInfo),
    Repository(RepositoryHandle),
    Repositories(BTreeMap<String, RepositoryHandle>),
    SocketAddrs(Vec<SocketAddr>),
    StorageSize(StorageSize),
    String(String),
    Strings(Vec<String>),
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl TryFrom<Response> for () {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::None => Ok(()),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<bool> for Response {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl TryFrom<Response> for bool {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::Bool(value) => Ok(value),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<PathBuf> for Response {
    fn from(value: PathBuf) -> Self {
        Self::Path(value)
    }
}

impl<'a> From<&'a Path> for Response {
    fn from(value: &'a Path) -> Self {
        Self::Path(value.to_owned())
    }
}

impl From<String> for Response {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl<'a> From<&'a str> for Response {
    fn from(value: &'a str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Vec<String>> for Response {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

impl From<RepositoryHandle> for Response {
    fn from(value: RepositoryHandle) -> Self {
        Self::Repository(value)
    }
}

impl From<BTreeMap<String, RepositoryHandle>> for Response {
    fn from(value: BTreeMap<String, RepositoryHandle>) -> Self {
        Self::Repositories(value)
    }
}

impl From<Vec<PeerInfo>> for Response {
    fn from(value: Vec<PeerInfo>) -> Self {
        Self::PeerInfo(value)
    }
}

impl From<Vec<PeerAddr>> for Response {
    fn from(value: Vec<PeerAddr>) -> Self {
        Self::PeerAddrs(value)
    }
}

impl From<Vec<SocketAddr>> for Response {
    fn from(value: Vec<SocketAddr>) -> Self {
        Self::SocketAddrs(value)
    }
}

impl From<StorageSize> for Response {
    fn from(value: StorageSize) -> Self {
        Self::StorageSize(value)
    }
}

impl From<QuotaInfo> for Response {
    fn from(value: QuotaInfo) -> Self {
        Self::QuotaInfo(value)
    }
}

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}
