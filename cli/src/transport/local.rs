//! Client and Server than run in different processes on the same device.

use crate::{
    handler::local::LocalHandler,
    protocol::{Error, Request, Response},
};
use interprocess::local_socket::{
    tokio::{Listener, Stream},
    traits::tokio::{Listener as _, Stream as _},
    GenericFilePath, ListenerOptions, ToFsName,
};
use ouisync_bridge::{
    protocol::SessionCookie,
    transport::{socket_server_connection, SocketClient},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use tokio::task::JoinSet;
use tokio_util::codec::{length_delimited::LengthDelimitedCodec, Framed};
use tracing::Instrument;

pub(crate) struct LocalServer {
    listener: Listener,
    path: PathBuf,
}

impl LocalServer {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();

        let listener = path
            .to_fs_name::<GenericFilePath>()
            .and_then(|name| ListenerOptions::new().name(name).create_tokio())
            .map_err(|error| {
                tracing::error!(
                    ?error,
                    "Failed to bind local API server to {}",
                    path.display(),
                );

                error
            })?;

        tracing::info!("Local API server listening on {}", path.display());

        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub async fn run(self, handler: LocalHandler) {
        let mut connections = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok(socket) => {
                    let socket = make_socket(socket);
                    connections.spawn(
                        socket_server_connection::run(
                            socket,
                            handler.clone(),
                            SessionCookie::DUMMY,
                        )
                        .instrument(tracing::info_span!("local client")),
                    );
                }
                Err(error) => {
                    tracing::error!(?error, "Failed to accept client");
                    break;
                }
            }
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            tracing::error!(?error, path = ?self.path, "Failed to remove socket");
        }
    }
}

pub(crate) struct LocalClient {
    inner: SocketClient<Socket, Request, Response, Error>,
}

impl LocalClient {
    pub async fn connect(name: impl AsRef<Path>) -> io::Result<Self> {
        let socket = Stream::connect(name.as_ref().to_fs_name::<GenericFilePath>()?).await?;
        let socket = make_socket(socket);

        Ok(Self {
            inner: SocketClient::new(socket),
        })
    }

    pub async fn invoke(&self, request: Request) -> Result<Response, Error> {
        self.inner.invoke(request).await
    }
}

type Socket = Framed<Stream, LengthDelimitedCodec>;

fn make_socket(inner: Stream) -> Socket {
    Framed::new(inner, LengthDelimitedCodec::new())
}
