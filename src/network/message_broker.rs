use super::{
    client::Client,
    message::{Message, Request, Response},
    object_stream::{ObjectReader, ObjectStream, ObjectWriter},
    server::Server,
};
use crate::Index;
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
    pub async fn recv(&mut self) -> Option<Request> {
        self.rx.recv().await
    }

    pub async fn send(&mut self, rs: Response) -> Result<(), SendError<Response>> {
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
    pub async fn recv(&mut self) -> Option<Response> {
        self.rx.recv().await
    }

    pub async fn send(&mut self, rq: Request) -> Result<(), SendError<Request>> {
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
    pub fn new(index: Index, stream: ObjectStream, on_finish: OnFinish) -> Self {
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

        task::spawn(inner.run(index, command_rx, request_rx, response_rx, finish_rx));

        Self {
            command_tx,
            _finish_tx: finish_tx,
        }
    }

    pub async fn add_connection(&self, stream: ObjectStream) {
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
    writers: Vec<ObjectWriter>,
    reader_count: usize,
    on_finish: OnFinish,
}

impl Inner {
    async fn run(
        mut self,
        index: Index,
        command_rx: mpsc::Receiver<Command>,
        request_rx: mpsc::Receiver<Request>,
        response_rx: mpsc::Receiver<Response>,
        finish_rx: oneshot::Receiver<()>,
    ) {
        let mut client = Client {};
        let client_stream = ClientStream {
            tx: self.command_tx.clone(),
            rx: response_rx,
        };

        let mut server = Server {};
        let server_stream = ServerStream {
            tx: self.command_tx.clone(),
            rx: request_rx,
        };

        select! {
            _ = self.handle_commands(command_rx) => (),
            _ = client.run(client_stream, &index) => (),
            _ = server.run(server_stream, &index) => (),
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

    fn handle_add_connection(&mut self, stream: ObjectStream) {
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
    mut reader: ObjectReader,
    command_tx: mpsc::Sender<Command>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) {
    loop {
        select! {
            result = reader.read() => {
                match result {
                    Ok(Message::Request(request)) => {
                        let _ = request_tx.send(request).await;
                    }
                    Ok(Message::Response(response)) => {
                        let _ = response_tx.send(response).await;
                    }
                    Err(_) => {
                        let _ = command_tx.send(Command::CloseReader).await;
                        break;
                    }
                }
            }
            _ = command_tx.closed() => break,
        }
    }
}

enum Command {
    AddConnection(ObjectStream),
    SendMessage(Message),
    CloseReader,
}

impl Command {
    fn into_send_message(self) -> Message {
        match self {
            Self::SendMessage(message) => message,
            _ => panic!("Command is not SendMessage"),
        }
    }
}
