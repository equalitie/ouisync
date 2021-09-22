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
pub(crate) struct ServerStream {
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
pub(crate) struct ClientStream {
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
            request_tx,
            response_tx,
            reader: MultiReader::new(),
            writer: MultiWriter::new(),
        };

        inner.handle_add_connection(stream);

        let client = Client::new(
            index.clone(),
            their_replica_id,
            ClientStream::new(command_tx.clone(), response_rx),
        );

        let server_stream = ServerStream::new(command_tx.clone(), request_rx);
        let server = Server::new(index, server_stream).await;

        task::spawn(inner.run(client, server, command_rx, finish_rx, on_finish));

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
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
    reader: MultiReader,
    writer: MultiWriter,
}

impl Inner {
    async fn run(
        mut self,
        mut client: Client,
        mut server: Server,
        command_rx: mpsc::Receiver<Command>,
        finish_rx: oneshot::Receiver<()>,
        on_finish: OnFinish,
    ) {
        select! {
            _ = self.handle_input(command_rx) => (),
            _ = log_error(client.run(), "client failed: ") => (),
            _ = log_error(server.run(), "server failed: ") => (),
            _ = finish_rx => (),
        }

        on_finish.await
    }

    async fn handle_input(&mut self, mut command_rx: mpsc::Receiver<Command>) {
        let mut run = true;

        while run {
            run = select! {
                command = command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command).await
                    } else {
                        false
                    }
                }
                message = self.reader.read() => {
                    if let Some(message) = message {
                        self.handle_recv_message(message).await
                    } else {
                        false
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::AddConnection(stream) => {
                self.handle_add_connection(stream);
                true
            }
            Command::SendMessage(message) => self.handle_send_message(message).await,
        }
    }

    fn handle_add_connection(&mut self, stream: TcpObjectStream) {
        let (reader, writer) = stream.into_split();
        self.reader.add(reader);
        self.writer.add(writer);
    }

    async fn handle_send_message(&mut self, message: Message) -> bool {
        self.writer.write(&message).await
    }

    async fn handle_recv_message(&self, message: Message) -> bool {
        match message {
            Message::Request(request) => self.request_tx.send(request).await.is_ok(),
            Message::Response(response) => self.response_tx.send(response).await.is_ok(),
        }
    }
}

pub(super) enum Command {
    AddConnection(TcpObjectStream),
    SendMessage(Message),
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

/// Wrapper for arbitrary number of `TcpObjectReader`s which reads from all of them simultaneously.
struct MultiReader {
    tx: mpsc::Sender<Option<Message>>,
    rx: mpsc::Receiver<Option<Message>>,
    count: usize,
}

impl MultiReader {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self { tx, rx, count: 0 }
    }

    fn add(&mut self, mut reader: TcpObjectReader) {
        let tx = self.tx.clone();
        self.count += 1;

        task::spawn(async move {
            loop {
                select! {
                    result = reader.read() => {
                        if let Ok(message) = result {
                            tx.send(Some(message)).await.unwrap_or(())
                        } else {
                            tx.send(None).await.unwrap_or(());
                            break;
                        }
                    },
                    _ = tx.closed() => break,
                }
            }
        });
    }

    async fn read(&mut self) -> Option<Message> {
        loop {
            if self.count == 0 {
                return None;
            }

            match self.rx.recv().await {
                Some(Some(message)) => return Some(message),
                Some(None) => {
                    self.count -= 1;
                }
                None => {
                    // This would mean that all senders were closed, but that can't happen because
                    // `self.tx` still exists.
                    unreachable!()
                }
            }
        }
    }
}

/// Wrapper for arbitrary number of `TcpObjectWriter`s which writes to the first available one.
struct MultiWriter {
    writers: Vec<TcpObjectWriter>,
}

impl MultiWriter {
    fn new() -> Self {
        Self {
            writers: Vec::new(),
        }
    }

    fn add(&mut self, writer: TcpObjectWriter) {
        self.writers.push(writer)
    }

    async fn write(&mut self, message: &Message) -> bool {
        while let Some(writer) = self.writers.last_mut() {
            if writer.write(message).await.is_ok() {
                return true;
            }

            self.writers.pop();
        }

        false
    }
}
