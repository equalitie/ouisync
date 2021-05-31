use super::{
    client::Client,
    message::{Message, Request, Response},
    object_stream::{ObjectReader, ObjectStream, ObjectWriter},
    server::Server,
};
use crate::{
    scoped_task_set::{ScopedTaskHandle, ScopedTaskSet},
    Index,
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{
    mpsc::{self, error::SendError, Receiver, Sender},
    Mutex,
};

/// A stream for receiving Requests and sending Responses
pub struct ServerStream {
    sender: Sender<Command>,
    receiver: Receiver<Request>,
}

impl ServerStream {
    pub async fn read(&mut self) -> Option<Request> {
        self.receiver.recv().await
    }

    pub async fn write(&mut self, rs: Response) -> Result<(), SendError<Response>> {
        self.sender
            .send(Command::SendMessage(Message::Response(rs)))
            .await
            .map_err(|e| SendError(e.0.into_send_message().into_response()))
    }
}

/// A stream for sending Requests and receiving Responses
pub struct ClientStream {
    sender: Sender<Command>,
    receiver: Receiver<Response>,
}

impl ClientStream {
    pub async fn read(&mut self) -> Option<Response> {
        self.receiver.recv().await
    }

    pub async fn write(&mut self, rq: Request) -> Result<(), SendError<Request>> {
        self.sender
            .send(Command::SendMessage(Message::Request(rq)))
            .await
            .map_err(|e| SendError(e.0.into_send_message().into_request()))
    }
}

type OnFinish = Pin<Box<dyn Future<Output = ()> + Send>>;

struct State {
    on_finish: Option<OnFinish>,
    receiver_count: u32,
}

/// Maintains one or more connections to a peer, listening on all of them at the same time. Note
/// that at the present all the connections are TCP based and so dropping some of them would make
/// sense. However, in the future we may also have other transports (e.g. Bluetooth) and thus
/// keeping all may make sence because even if one is dropped, the others may still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
pub struct MessageBroker {
    inner: Arc<Inner>,
    command_tx: Sender<Command>,
    tasks: ScopedTaskSet,
}

impl MessageBroker {
    pub fn new(index: Index, conn: ObjectStream, on_finish: OnFinish) -> Self {
        let tasks = ScopedTaskSet::default();

        let (command_tx, command_rx) = mpsc::channel(1);
        let (request_tx, request_rx) = mpsc::channel(1);
        let (response_tx, response_rx) = mpsc::channel(1);

        let inner = Arc::new(Inner {
            request_tx,
            response_tx,
            state: Mutex::new(State {
                on_finish: Some(on_finish),
                receiver_count: 0,
            }),
            task_handle: tasks.handle().clone(),
            index,
        });

        {
            let inner = inner.clone();
            let command_tx = command_tx.clone();
            tasks.spawn(async move {
                let mut client = Client {};
                let stream = ClientStream {
                    sender: command_tx,
                    receiver: response_rx,
                };
                client.run(stream, &inner.index).await;
                inner.finish().await;
            })
        }

        {
            let inner = inner.clone();
            let command_tx = command_tx.clone();
            tasks.spawn(async move {
                let mut server = Server {};
                let stream = ServerStream {
                    sender: command_tx,
                    receiver: request_rx,
                };
                server.run(stream, &inner.index).await;
                inner.finish().await;
            })
        }

        let (reader, writer) = conn.into_split();

        tasks.spawn(inner.clone().run_command_handler(command_rx, writer));
        tasks.spawn(inner.clone().run_message_receiver(reader));

        Self {
            inner,
            command_tx,
            tasks,
        }
    }

    pub async fn add_connection(&self, conn: ObjectStream) {
        let (reader, writer) = conn.into_split();

        if self
            .command_tx
            .send(Command::AddWriter(writer))
            .await
            .is_err()
        {
            log::error!("Failed to activate writer");
            return;
        }

        self.tasks
            .spawn(self.inner.clone().run_message_receiver(reader));
    }
}

struct Inner {
    request_tx: Sender<Request>,
    response_tx: Sender<Response>,
    state: Mutex<State>,
    task_handle: ScopedTaskHandle,
    index: Index,
}

impl Inner {
    async fn run_command_handler(self: Arc<Self>, mut rx: Receiver<Command>, writer: ObjectWriter) {
        let mut writers = vec![writer];

        while let Some(command) = rx.recv().await {
            match command {
                Command::AddWriter(writer) => {
                    writers.push(writer);
                }
                Command::SendMessage(message) => {
                    assert!(!writers.is_empty());

                    while !writers.is_empty() {
                        if writers[0].write(&message).await.is_ok() {
                            break;
                        }
                        writers.remove(0);
                    }

                    if writers.is_empty() {
                        self.finish().await;
                    }
                }
            }
        }
    }

    async fn run_message_receiver(self: Arc<Self>, mut reader: ObjectReader) {
        self.state.lock().await.receiver_count += 1;

        loop {
            match reader.read().await {
                Ok(msg) => {
                    self.handle_message(msg).await;
                }
                Err(_) => {
                    let rc = {
                        let mut state = self.state.lock().await;
                        state.receiver_count -= 1;
                        state.receiver_count
                    };

                    if rc == 0 {
                        self.finish().await;
                    }
                    break;
                }
            };
        }
    }

    async fn handle_message(&self, msg: Message) {
        match msg {
            Message::Request(rq) => {
                let _ = self.request_tx.send(rq).await;
            }
            Message::Response(rs) => {
                let _ = self.response_tx.send(rs).await;
            }
        }
    }

    async fn finish(&self) {
        let on_finish = self.state.lock().await.on_finish.take();
        if let Some(on_finish) = on_finish {
            on_finish.await
        }

        self.task_handle.abort_all();
    }
}

enum Command {
    AddWriter(ObjectWriter),
    SendMessage(Message),
}

impl Command {
    fn into_send_message(self) -> Message {
        match self {
            Command::SendMessage(m) => m,
            _ => panic!("Command is not SendMessage"),
        }
    }
}
