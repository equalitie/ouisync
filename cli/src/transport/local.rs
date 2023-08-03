//! Client and Server than run in different processes on the same device.

use crate::{
    handler::local::LocalHandler,
    protocol::{Request, Response},
};
use interprocess::local_socket::{
    tokio::{LocalSocketListener, LocalSocketStream},
    ToLocalSocketName,
};
use ouisync_bridge::{
    error::Result,
    transport::{socket_server_connection, SocketClient},
};
use std::{fs, io, path::PathBuf};
use tokio::task::JoinSet;
use tokio_util::{
    codec::{length_delimited::LengthDelimitedCodec, Framed},
    compat::{Compat, FuturesAsyncReadCompatExt},
};
use tracing::Instrument;

pub(crate) struct LocalServer {
    listener: LocalSocketListener,
    path: Option<PathBuf>,
}

impl LocalServer {
    pub fn bind<'a>(name: impl ToLocalSocketName<'a> + Clone) -> io::Result<Self> {
        let orig_name = name.clone();
        let name = name.to_local_socket_name()?;

        let listener = LocalSocketListener::bind(orig_name).map_err(|error| {
            tracing::error!(
                ?error,
                "Failed to bind local API server to {}",
                name.inner().to_string_lossy()
            );

            error
        })?;

        let path = if name.is_path() {
            Some(name.inner().into())
        } else {
            None
        };

        tracing::info!(
            "Local API server listening on {}",
            name.inner().to_string_lossy()
        );

        Ok(Self { listener, path })
    }

    pub async fn run(self, handler: LocalHandler) {
        let mut connections = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok(socket) => {
                    let socket = make_socket(socket);
                    connections.spawn(
                        socket_server_connection::run(socket, handler.clone())
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
        if let Some(path) = &self.path {
            if let Err(error) = fs::remove_file(path) {
                tracing::error!(?error, ?path, "Failed to remove socket");
            }
        }
    }
}

pub(crate) struct LocalClient {
    inner: SocketClient<Socket, Request, Response>,
}

impl LocalClient {
    pub async fn connect<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        let socket = LocalSocketStream::connect(name).await?;
        let socket = make_socket(socket);

        Ok(Self {
            inner: SocketClient::new(socket),
        })
    }

    pub async fn invoke(&self, request: Request) -> Result<Response> {
        self.inner.invoke(request).await
    }
}

type Socket = Framed<Compat<LocalSocketStream>, LengthDelimitedCodec>;

fn make_socket(inner: LocalSocketStream) -> Socket {
    Framed::new(inner.compat(), LengthDelimitedCodec::new())
}
