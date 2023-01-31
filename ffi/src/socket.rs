use futures_util::{SinkExt, StreamExt};
use std::{
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};

pub(crate) enum Stream {
    Memory(memory::ServerStream),
    Ws(ws::Stream),
}

impl From<memory::ServerStream> for Stream {
    fn from(inner: memory::ServerStream) -> Self {
        Self::Memory(inner)
    }
}

impl From<ws::Stream> for Stream {
    fn from(inner: ws::Stream) -> Self {
        Self::Ws(inner)
    }
}

impl futures_util::Stream for Stream {
    type Item = io::Result<Vec<u8>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Memory(stream) => Pin::new(stream).poll_next(cx),
            Self::Ws(stream) => Pin::new(stream).poll_next(cx),
        }
    }
}

impl futures_util::Sink<Vec<u8>> for Stream {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Memory(stream) => Pin::new(stream).poll_ready(cx),
            Self::Ws(stream) => Pin::new(stream).poll_ready(cx),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        match self.get_mut() {
            Self::Memory(stream) => Pin::new(stream).start_send(item),
            Self::Ws(stream) => Pin::new(stream).start_send(item),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Memory(stream) => Pin::new(stream).poll_flush(cx),
            Self::Ws(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Memory(stream) => Pin::new(stream).poll_close(cx),
            Self::Ws(stream) => Pin::new(stream).poll_close(cx),
        }
    }
}

pub(crate) mod memory {
    use super::*;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::UnboundedReceiverStream;
    use tokio_util::sync::PollSender;

    pub struct ServerStream {
        tx: PollSender<Vec<u8>>,
        rx: UnboundedReceiverStream<Vec<u8>>,
    }

    pub type ClientSender = mpsc::UnboundedSender<Vec<u8>>;
    pub type ClientReceiver = mpsc::Receiver<Vec<u8>>;

    pub fn new() -> (ServerStream, ClientSender, ClientReceiver) {
        let (server_tx, client_rx) = mpsc::channel(1);
        let (client_tx, server_rx) = mpsc::unbounded_channel();

        let server = ServerStream {
            tx: PollSender::new(server_tx),
            rx: UnboundedReceiverStream::new(server_rx),
        };

        (server, client_tx, client_rx)
    }

    impl futures_util::Stream for ServerStream {
        type Item = io::Result<Vec<u8>>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(ready!(self.rx.poll_next_unpin(cx)).map(Ok))
        }
    }

    impl futures_util::Sink<Vec<u8>> for ServerStream {
        type Error = io::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(
                ready!(self.tx.poll_ready_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
            )
        }

        fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
            self.tx
                .start_send_unpin(item)
                .map_err(|_| io::ErrorKind::BrokenPipe.into())
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(
                ready!(self.tx.poll_flush_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
            )
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(
                ready!(self.tx.poll_close_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
            )
        }
    }
}

pub(crate) mod ws {
    use super::*;
    use std::net::SocketAddr;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{
        tungstenite::{self, Message},
        WebSocketStream,
    };

    pub struct Listener(TcpListener);

    impl Listener {
        pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
            let listener = TcpListener::bind(addr).await?;

            match listener.local_addr() {
                Ok(addr) => tracing::debug!("websocket server bound to {:?}", addr),
                Err(error) => {
                    tracing::error!(?error, "failed to retrieve websocket server local address")
                }
            }

            Ok(Self(listener))
        }

        pub async fn accept(&self) -> io::Result<Stream> {
            match self.0.accept().await {
                Ok((stream, addr)) => {
                    // Convert to websocket
                    let stream = match tokio_tungstenite::accept_async(stream).await {
                        Ok(stream) => stream,
                        Err(error) => {
                            tracing::error!(
                                ?error,
                                "failed to upgrade tcp stream to websocket stream"
                            );
                            return Err(into_io_error(error));
                        }
                    };

                    tracing::debug!("websocket client accepted at {:?}", addr);

                    Ok(Stream(stream))
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept websocket client");
                    Err(error)
                }
            }
        }
    }

    pub struct Stream(WebSocketStream<TcpStream>);

    impl futures_util::Stream for Stream {
        type Item = io::Result<Vec<u8>>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            loop {
                match ready!(self.0.poll_next_unpin(cx)) {
                    Some(Ok(Message::Binary(payload))) => return Poll::Ready(Some(Ok(payload))),
                    Some(Ok(
                        message @ (Message::Text(_)
                        | Message::Ping(_)
                        | Message::Pong(_)
                        | Message::Close(_)
                        | Message::Frame(_)),
                    )) => {
                        tracing::debug!(?message, "unexpected message type");
                        continue;
                    }
                    Some(Err(error)) => {
                        return Poll::Ready(Some(Err(into_io_error(error))));
                    }
                    None => return Poll::Ready(None),
                }
            }
        }
    }

    impl futures_util::Sink<Vec<u8>> for Stream {
        type Error = io::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(ready!(self.0.poll_ready_unpin(cx)).map_err(into_io_error))
        }

        fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
            self.0
                .start_send_unpin(Message::Binary(item))
                .map_err(into_io_error)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(ready!(self.0.poll_flush_unpin(cx)).map_err(into_io_error))
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(ready!(self.0.poll_close_unpin(cx)).map_err(into_io_error))
        }
    }

    fn into_io_error(src: tungstenite::Error) -> io::Error {
        io::Error::new(io::ErrorKind::Other, src)
    }
}
