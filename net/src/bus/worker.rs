use super::{
    dispatch::Dispatcher,
    topic::{Input, Output, TopicId, TopicNonce, TopicState},
};
use crate::unified::{Connection, RecvStream, SendStream};
use futures_util::{future, stream::FuturesUnordered, StreamExt};
use std::io;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    sync::{mpsc, oneshot},
    task,
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

pub(super) async fn run(connection: Connection, mut command_rx: mpsc::UnboundedReceiver<Command>) {
    let dispatcher = Dispatcher::new(connection);
    let mut topic_tasks = FuturesUnordered::new();

    loop {
        let command = select! {
            command = command_rx.recv() => {
                if let Some(command) = command {
                    command
                } else {
                    break;
                }
            }
            Some(_) = topic_tasks.next() => continue,
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

    match try_create_topic(topic_id, TopicNonce::random(), dispatcher).await {
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

#[instrument(name = "topic", skip(dispatcher))]
async fn try_create_topic(
    topic_id: TopicId,
    nonce: TopicNonce,
    dispatcher: &Dispatcher,
) -> io::Result<(SendStream, RecvStream)> {
    let mut state = TopicState::new(nonce);
    let mut tasks = FuturesUnordered::new();
    let mut incoming = None;
    let mut outgoing = None;
    let mut last_error = None;

    loop {
        while let Some(output) = state.poll() {
            match output {
                Output::OutgoingCreate(nonce) => {
                    tasks.push(create_stream(
                        dispatcher,
                        CreateInput::Outgoing(topic_id, nonce),
                    ));
                }
                Output::OutgoingAccept => match outgoing {
                    Some(streams) => return Ok(streams),
                    None => unreachable!(),
                },
                Output::IncomingCreate => {
                    tasks.push(create_stream(dispatcher, CreateInput::Incoming(topic_id)));
                }
                Output::IncomingAccept => match incoming {
                    Some(streams) => return Ok(streams),
                    None => unreachable!(),
                },
            }
        }

        // Make sure we yield to the runtime at least once here to prevent busy looping. This can
        // happen when we successfully created one of the streams (incoming or outgoing) and then
        // the connection fails. Trying to create the other steam afterwards might immediately
        // fail without yielding and if we then tried to create the stream again we would enter
        // infinite busy loop, not even giving the runtime chance to cancel this task.
        let (Some(output), _) = future::join(tasks.next(), task::yield_now()).await else {
            break;
        };

        match output {
            CreateOutput::Incoming(Ok((nonce, send_stream, recv_stream))) => {
                incoming = Some((send_stream, recv_stream));
                state.handle(Input::IncomingCreated(nonce));
            }
            CreateOutput::Incoming(Err(error)) => {
                last_error = Some(error);
                state.handle(Input::IncomingFailed);
            }
            CreateOutput::Outgoing(Ok((send_stream, recv_stream))) => {
                outgoing = Some((send_stream, recv_stream));
                state.handle(Input::OutgoingCreated);
            }
            CreateOutput::Outgoing(Err(error)) => {
                last_error = Some(error);
                state.handle(Input::OutgoingFailed);
            }
        }
    }

    return Err(last_error.unwrap_or_else(|| io::ErrorKind::ConnectionAborted.into()));
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
