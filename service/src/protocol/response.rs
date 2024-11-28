use crate::repository::RepositoryHandle;
use ouisync::{PeerAddr, PeerInfo, ShareToken, StorageSize};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[expect(clippy::large_enum_variant)]
pub enum Response {
    None,
    Bool(bool),
    Duration(Duration),
    Path(PathBuf),
    PeerAddrs(Vec<PeerAddr>),
    PeerInfo(Vec<PeerInfo>),
    QuotaInfo(QuotaInfo),
    Repository(RepositoryHandle),
    Repositories(BTreeMap<String, RepositoryHandle>),
    ShareToken(ShareToken),
    SocketAddr(SocketAddr),
    SocketAddrs(Vec<SocketAddr>),
    StorageSize(StorageSize),
    U32(u32),
}

macro_rules! impl_response_conversion {
    ($variant:ident ( $ty:ty )) => {
        impl From<$ty> for Response {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }

        impl TryFrom<Response> for $ty {
            type Error = UnexpectedResponse;

            fn try_from(response: Response) -> Result<Self, Self::Error> {
                match response {
                    Response::$variant(value) => Ok(value),
                    _ => Err(UnexpectedResponse),
                }
            }
        }

        impl TryFrom<Response> for Option<$ty> {
            type Error = UnexpectedResponse;

            fn try_from(value: Response) -> Result<Self, Self::Error> {
                match value {
                    Response::$variant(value) => Ok(Some(value)),
                    Response::None => Ok(None),
                    _ => Err(UnexpectedResponse),
                }
            }
        }
    };
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

impl<T> From<Option<T>> for Response
where
    Self: From<T>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::from(value),
            None => Self::None,
        }
    }
}

impl<'a> From<&'a Path> for Response {
    fn from(value: &'a Path) -> Self {
        Self::Path(value.to_owned())
    }
}

impl_response_conversion!(Bool(bool));
impl_response_conversion!(Duration(Duration));
impl_response_conversion!(Path(PathBuf));
impl_response_conversion!(Repository(RepositoryHandle));
impl_response_conversion!(Repositories(BTreeMap<String, RepositoryHandle>));
impl_response_conversion!(PeerInfo(Vec<PeerInfo>));
impl_response_conversion!(PeerAddrs(Vec<PeerAddr>));
impl_response_conversion!(ShareToken(ShareToken));
impl_response_conversion!(SocketAddr(SocketAddr));
impl_response_conversion!(SocketAddrs(Vec<SocketAddr>));
impl_response_conversion!(StorageSize(StorageSize));
impl_response_conversion!(QuotaInfo(QuotaInfo));
impl_response_conversion!(U32(u32));

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}
