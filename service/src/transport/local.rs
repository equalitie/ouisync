use super::common::{Reader, Writer};
use crate::protocol::Request;
use interprocess::local_socket::{
    tokio::{Listener, Stream as Socket},
    traits::tokio::{Listener as _, Stream as _},
    GenericFilePath, ListenerOptions, ToFsName,
};
use std::{io, path::Path};

use crate::protocol::ServerPayload;

pub(crate) struct LocalServer {
    listener: Listener,
}

impl LocalServer {
    pub async fn bind(socket_path: &Path) -> io::Result<Self> {
        let listener = socket_path
            .to_fs_name::<GenericFilePath>()
            .and_then(|name| {
                ListenerOptions::new()
                    .name(name)
                    .reclaim_name(true)
                    .create_tokio()
            })?;

        Ok(Self { listener })
    }

    pub async fn accept(&self) -> io::Result<(LocalServerReader, LocalServerWriter)> {
        let stream = self.listener.accept().await?;
        let (reader, writer) = stream.split();

        let reader = Reader::new(reader);
        let writer = Writer::new(writer);

        Ok((reader, writer))
    }
}

pub async fn connect(socket_path: &Path) -> io::Result<(LocalClientReader, LocalClientWriter)> {
    let socket = Socket::connect(socket_path.to_fs_name::<GenericFilePath>()?).await?;
    let (reader, writer) = socket.split();

    let reader = Reader::new(reader);
    let writer = Writer::new(writer);

    Ok((reader, writer))
}

pub(crate) type LocalServerReader = Reader<Request>;
pub(crate) type LocalServerWriter = Writer<ServerPayload>;

pub type LocalClientReader = Reader<ServerPayload>;
pub type LocalClientWriter = Writer<Request>;
