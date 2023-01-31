pub(crate) mod ws {
    use std::{
        io,
        net::SocketAddr,
        pin::Pin,
        task::{Context, Poll},
    };
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
                match Pin::new(&mut self.0).poll_next(cx) {
                    Poll::Ready(Some(Ok(Message::Binary(payload)))) => {
                        return Poll::Ready(Some(Ok(payload)))
                    }
                    Poll::Ready(Some(Ok(
                        message @ (Message::Text(_)
                        | Message::Ping(_)
                        | Message::Pong(_)
                        | Message::Close(_)
                        | Message::Frame(_)),
                    ))) => {
                        tracing::debug!(?message, "unexpected message type");
                        continue;
                    }
                    Poll::Ready(Some(Err(error))) => {
                        return Poll::Ready(Some(Err(into_io_error(error))));
                    }
                    Poll::Ready(None) => return Poll::Ready(None),
                    Poll::Pending => return Poll::Pending,
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
            match Pin::new(&mut self.0).poll_ready(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => Poll::Ready(Err(into_io_error(error))),
                Poll::Pending => Poll::Pending,
            }
        }

        fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
            Pin::new(&mut self.0)
                .start_send(Message::Binary(item))
                .map_err(into_io_error)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            match Pin::new(&mut self.0).poll_flush(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => Poll::Ready(Err(into_io_error(error))),
                Poll::Pending => Poll::Pending,
            }
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            match Pin::new(&mut self.0).poll_close(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => Poll::Ready(Err(into_io_error(error))),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    fn into_io_error(src: tungstenite::Error) -> io::Error {
        io::Error::new(io::ErrorKind::Other, src)
    }
}
