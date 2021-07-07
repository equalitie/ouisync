use super::{
    client::Client,
    message::{Message, Request, Response},
    object_stream::{TcpObjectReader, TcpObjectStream, TcpObjectWriter},
    server::Server,
};
use crate::{error::Result, index::Index, replica_id::ReplicaId};
use std::{future::Future, pin::Pin};
use tokio::{
    select,
    sync::{
        mpsc::{self, error::SendError},
        oneshot,
    },
    task,
};

/// A stream for receiving Requests and sending Responses
pub struct ServerStream {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Request>,
}

impl ServerStream {
    pub(super) fn new(tx: mpsc::Sender<Command>, rx: mpsc::Receiver<Request>) -> Self {
        Self { tx, rx }
    }

    pub async fn recv(&mut self) -> Option<Request> {
        let rq = self.rx.recv().await?;
        log::trace!("server: recv {:?}", rq);
        Some(rq)
    }

    pub async fn send(&self, rs: Response) -> Result<(), SendError<Response>> {
        log::trace!("server: send {:?}", rs);
        self.tx
            .send(Command::SendMessage(Message::Response(rs)))
            .await
            .map_err(|e| SendError(into_message(e.0)))
    }
}

/// A stream for sending Requests and receiving Responses
pub struct ClientStream {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Response>,
}

impl ClientStream {
    pub(super) fn new(tx: mpsc::Sender<Command>, rx: mpsc::Receiver<Response>) -> Self {
        Self { tx, rx }
    }

    pub async fn recv(&mut self) -> Option<Response> {
        let rs = self.rx.recv().await?;
        log::trace!("client: recv {:?}", rs);
        Some(rs)
    }

    pub async fn send(&self, rq: Request) -> Result<(), SendError<Request>> {
        log::trace!("client: send {:?}", rq);
        self.tx
            .send(Command::SendMessage(Message::Request(rq)))
            .await
            .map_err(|e| SendError(into_message(e.0)))
    }
}

fn into_message<T: From<Message>>(command: Command) -> T {
    command.into_send_message().into()
}

type OnFinish = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Maintains one or more connections to a peer, listening on all of them at the same time. Note
/// that at the present all the connections are TCP based and so dropping some of them would make
/// sense. However, in the future we may also have other transports (e.g. Bluetooth) and thus
/// keeping all may make sence because even if one is dropped, the others may still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
pub struct MessageBroker {
    command_tx: mpsc::Sender<Command>,
    _finish_tx: oneshot::Sender<()>,
}

impl MessageBroker {
    pub async fn new(
        index: Index,
        their_replica_id: ReplicaId,
        stream: TcpObjectStream,
        on_finish: OnFinish,
    ) -> Self {
        // Channel party!
        let (command_tx, command_rx) = mpsc::channel(1);
        let (request_tx, request_rx) = mpsc::channel(1);
        let (response_tx, response_rx) = mpsc::channel(1);
        let (finish_tx, finish_rx) = oneshot::channel();

        let mut inner = Inner {
            command_tx: command_tx.clone(),
            request_tx,
            response_tx,
            writers: Vec::new(),
            reader_count: 0,
            on_finish,
        };

        inner.handle_add_connection(stream);

        let client = Client::new(
            index.clone(),
            their_replica_id,
            ClientStream::new(command_tx.clone(), response_rx),
        );

        let server_stream = ServerStream::new(command_tx.clone(), request_rx);
        let server = Server::new(index, server_stream).await;

        task::spawn(inner.run(client, server, command_rx, finish_rx));

        Self {
            command_tx,
            _finish_tx: finish_tx,
        }
    }

    pub async fn add_connection(&self, stream: TcpObjectStream) {
        if self
            .command_tx
            .send(Command::AddConnection(stream))
            .await
            .is_err()
        {
            log::error!("Failed to add connection - broker already finished");
        }
    }
}

struct Inner {
    command_tx: mpsc::Sender<Command>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
    writers: Vec<TcpObjectWriter>,
    reader_count: usize,
    on_finish: OnFinish,
}

impl Inner {
    async fn run(
        mut self,
        mut client: Client,
        mut server: Server,
        command_rx: mpsc::Receiver<Command>,
        finish_rx: oneshot::Receiver<()>,
    ) {
        select! {
            _ = self.handle_commands(command_rx) => (),
            _ = log_error(client.run(), "client failed: ") => (),
            _ = log_error(server.run(), "server failed: ") => (),
            _ = finish_rx => (),
        }

        self.on_finish.await
    }

    async fn handle_commands(&mut self, mut command_rx: mpsc::Receiver<Command>) {
        loop {
            if self.writers.is_empty() || self.reader_count == 0 {
                break;
            }

            match command_rx.recv().await {
                Some(Command::AddConnection(stream)) => self.handle_add_connection(stream),
                Some(Command::SendMessage(message)) => self.handle_send_message(message).await,
                Some(Command::CloseReader) => self.reader_count -= 1,
                None => break,
            }
        }
    }

    fn handle_add_connection(&mut self, stream: TcpObjectStream) {
        let (reader, writer) = stream.into_split();
        self.writers.push(writer);
        self.reader_count += 1;

        task::spawn(read(
            reader,
            self.command_tx.clone(),
            self.request_tx.clone(),
            self.response_tx.clone(),
        ));
    }

    async fn handle_send_message(&mut self, message: Message) {
        while !self.writers.is_empty() {
            if self.writers[0].write(&message).await.is_ok() {
                break;
            }

            self.writers.remove(0);
        }
    }
}

async fn read(
    mut reader: TcpObjectReader,
    command_tx: mpsc::Sender<Command>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) {
    loop {
        select! {
            result = reader.read() => {
                match result {
                    Ok(Message::Request(request)) => {
                        request_tx.send(request).await.unwrap_or(())
                    }
                    Ok(Message::Response(response)) => {
                        response_tx.send(response).await.unwrap_or(())
                    }
                    Err(_) => {
                        command_tx.send(Command::CloseReader).await.unwrap_or(());
                        break;
                    }
                }
            }
            _ = command_tx.closed() => break,
        }
    }
}

pub(super) enum Command {
    AddConnection(TcpObjectStream),
    SendMessage(Message),
    CloseReader,
}

impl Command {
    pub(super) fn into_send_message(self) -> Message {
        match self {
            Self::SendMessage(message) => message,
            _ => panic!("Command is not SendMessage"),
        }
    }
}

async fn log_error<F>(fut: F, prefix: &'static str)
where
    F: Future<Output = Result<()>>,
{
    if let Err(error) = fut.await {
        log::error!("{}{}", prefix, error.verbose())
    }
}
