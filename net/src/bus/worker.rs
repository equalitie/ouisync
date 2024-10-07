use super::{
    dispatch::Dispatcher,
    topic::{Input, Output, TopicId, TopicNonce, TopicState},
};
use crate::unified::{Connection, RecvStream, SendStream};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::io;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    sync::{mpsc, oneshot},
};
use tracing::instrument;

pub(super) enum Command {
    Create {
        topic_id: TopicId,
        send_stream_tx: oneshot::Sender<io::Result<SendStream>>,
        recv_stream_tx: oneshot::Sender<io::Result<RecvStream>>,
    },
    Close {
        reply_tx: oneshot::Sender<()>,
    },
}

#[instrument(name = "worker", skip_all, fields(addr = %connection.remote_addr()))]
pub(super) async fn run(connection: Connection, mut command_rx: mpsc::UnboundedReceiver<Command>) {
    let dispatcher = Dispatcher::new(connection);
    let mut topic_tasks = FuturesUnordered::new();

    loop {
        let command = select! {
            Some(command) = command_rx.recv() => command,
            Some(_) = topic_tasks.next() => continue,
            _ = dispatcher.closed() => break,
        };

        match command {
            Command::Create {
                topic_id,
                send_stream_tx,
                recv_stream_tx,
            } => {
                topic_tasks.push(create_topic(
                    topic_id,
                    send_stream_tx,
                    recv_stream_tx,
                    &dispatcher,
                ));
            }
            Command::Close { reply_tx } => {
                dispatcher.close().await;
                reply_tx.send(()).ok();
                break;
            }
        }
    }
}

async fn create_topic(
    topic_id: TopicId,
    send_stream_tx: oneshot::Sender<io::Result<SendStream>>,
    recv_stream_tx: oneshot::Sender<io::Result<RecvStream>>,
    dispatcher: &Dispatcher,
) {
    // TODO: handle receiver cancellation

    match TopicHandler::new(topic_id, TopicNonce::random(), dispatcher)
        .run()
        .await
    {
        Ok((send_stream, recv_stream)) => {
            send_stream_tx.send(Ok(send_stream)).ok();
            recv_stream_tx.send(Ok(recv_stream)).ok();
        }
        Err(error) => {
            send_stream_tx
                .send(Err(io::ErrorKind::BrokenPipe.into()))
                .ok();
            recv_stream_tx.send(Err(error)).ok();
        }
    }
}

struct TopicHandler<'a> {
    topic_id: TopicId,
    state: TopicState,
    dispatcher: &'a Dispatcher,
}

impl<'a> TopicHandler<'a> {
    fn new(topic_id: TopicId, nonce: TopicNonce, dispatcher: &'a Dispatcher) -> Self {
        let state = TopicState::new(nonce);

        Self {
            topic_id,
            state,
            dispatcher,
        }
    }

    #[instrument(name = "topic", skip_all, fields(topic_id = ?self.topic_id, nonce = ?self.state.nonce))]
    async fn run(mut self) -> io::Result<(SendStream, RecvStream)> {
        let mut tasks = FuturesUnordered::new();
        let mut incoming = None;
        let mut outgoing = None;
        let mut last_error = None;

        loop {
            while let Some(output) = self.state.poll() {
                match output {
                    Output::OutgoingCreate(nonce) => {
                        tasks.push(create_stream(
                            self.dispatcher,
                            CreateInput::Outgoing(self.topic_id, nonce),
                        ));
                    }
                    Output::OutgoingAccept => match outgoing {
                        Some(streams) => return Ok(streams),
                        None => unreachable!(),
                    },
                    Output::IncomingCreate => {
                        tasks.push(create_stream(
                            self.dispatcher,
                            CreateInput::Incoming(self.topic_id),
                        ));
                    }
                    Output::IncomingAccept => match incoming {
                        Some(streams) => return Ok(streams),
                        None => unreachable!(),
                    },
                }
            }

            let Some(output) = tasks.next().await else {
                break;
            };

            match output {
                CreateOutput::Incoming(Ok((nonce, send_stream, recv_stream))) => {
                    incoming = Some((send_stream, recv_stream));
                    self.state.handle(Input::IncomingCreated(nonce));
                }
                CreateOutput::Incoming(Err(error)) => {
                    tracing::debug!(?error, "failed to create incoming stream");
                    last_error = Some(error);
                    self.state.handle(Input::IncomingFailed);
                }
                CreateOutput::Outgoing(Ok((send_stream, recv_stream))) => {
                    outgoing = Some((send_stream, recv_stream));
                    self.state.handle(Input::OutgoingCreated);
                }
                CreateOutput::Outgoing(Err(error)) => {
                    tracing::debug!(?error, "failed to create outgoing stream");
                    last_error = Some(error);
                    self.state.handle(Input::OutgoingFailed);
                }
            }
        }

        return Err(last_error.unwrap_or_else(|| io::ErrorKind::ConnectionAborted.into()));
    }
}

async fn create_stream(dispatcher: &Dispatcher, input: CreateInput) -> CreateOutput {
    match input {
        CreateInput::Incoming(topic_id) => {
            CreateOutput::Incoming(create_incoming_stream(dispatcher, topic_id).await)
        }
        CreateInput::Outgoing(topic_id, nonce) => {
            CreateOutput::Outgoing(create_outgoing_stream(dispatcher, topic_id, nonce).await)
        }
    }
}

async fn create_incoming_stream(
    dispatcher: &Dispatcher,
    topic_id: TopicId,
) -> io::Result<(TopicNonce, SendStream, RecvStream)> {
    let (mut send_stream, mut recv_stream) = dispatcher.incoming(topic_id).await?;

    let mut buffer = [0; TopicNonce::SIZE];
    recv_stream.read_exact(&mut buffer).await?;
    let nonce = TopicNonce::from(buffer);

    send_stream.write_all(ACK).await?;

    Ok((nonce, send_stream, recv_stream))
}

async fn create_outgoing_stream(
    dispatcher: &Dispatcher,
    topic_id: TopicId,
    nonce: TopicNonce,
) -> io::Result<(SendStream, RecvStream)> {
    let (mut send_stream, mut recv_stream) = dispatcher.outgoing(topic_id).await?;

    send_stream.write_all(nonce.as_bytes()).await?;

    let mut buffer = [0; ACK.len()];
    recv_stream.read_exact(&mut buffer).await?;

    if buffer != ACK {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ack"));
    }

    Ok((send_stream, recv_stream))
}

const ACK: &[u8] = &[0xac];

enum CreateInput {
    Incoming(TopicId),
    Outgoing(TopicId, TopicNonce),
}

enum CreateOutput {
    Incoming(io::Result<(TopicNonce, SendStream, RecvStream)>),
    Outgoing(io::Result<(SendStream, RecvStream)>),
}
