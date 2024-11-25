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
    Duration(Duration),
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

impl TryFrom<Response> for Option<Duration> {
    type Error = UnexpectedResponse;

    fn try_from(value: Response) -> Result<Self, Self::Error> {
        match value {
            Response::Duration(value) => Ok(Some(value)),
            Response::None => Ok(None),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl<'a> From<&'a Path> for Response {
    fn from(value: &'a Path) -> Self {
        Self::Path(value.to_owned())
    }
}

impl<'a> From<&'a str> for Response {
    fn from(value: &'a str) -> Self {
        Self::String(value.to_owned())
    }
}

impl_response_conversion!(Bool(bool));
impl_response_conversion!(Duration(Duration));
impl_response_conversion!(Path(PathBuf));
impl_response_conversion!(String(String));
impl_response_conversion!(Strings(Vec<String>));
impl_response_conversion!(Repository(RepositoryHandle));
impl_response_conversion!(Repositories(BTreeMap<String, RepositoryHandle>));
impl_response_conversion!(PeerInfo(Vec<PeerInfo>));
impl_response_conversion!(PeerAddrs(Vec<PeerAddr>));
impl_response_conversion!(SocketAddrs(Vec<SocketAddr>));
impl_response_conversion!(StorageSize(StorageSize));
impl_response_conversion!(QuotaInfo(QuotaInfo));

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}
