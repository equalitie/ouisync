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
    mpsc::{error::SendError, Receiver, Sender},
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
    tasks: ScopedTaskSet,
}

impl MessageBroker {
    pub fn new(index: Index, on_finish: OnFinish) -> Self {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(1);
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(1);
        let (response_tx, response_rx) = tokio::sync::mpsc::channel(1);

        let tasks = ScopedTaskSet::default();
        let task_handle = tasks.handle().clone();

        let inner = Arc::new(Inner {
            send_channel: command_tx,
            request_tx,
            response_tx,
            state: Mutex::new(State {
                on_finish: Some(on_finish),
                receiver_count: 0,
            }),
            task_handle,
            index,
        });

        tasks.spawn(
            inner
                .clone()
                .run_command_handler(command_rx, request_rx, response_rx),
        );

        Self { inner, tasks }
    }

    pub fn add_connection(&self, con: ObjectStream) {
        let inner = self.inner.clone();
        let send_channel = inner.send_channel.clone();

        self.tasks.spawn(async move {
            let (r, w) = con.into_split();

            if send_channel.send(Command::AddWriter(w)).await.is_err() {
                println!("Failed to activate writer");
                // XXX: Tell self to abort
                return;
            }

            inner.run_message_receiver(r).await;
        });
    }
}

struct Inner {
    send_channel: Sender<Command>,
    request_tx: Sender<Request>,
    response_tx: Sender<Response>,
    state: Mutex<State>,
    task_handle: ScopedTaskHandle,
    index: Index,
}

impl Inner {
    fn create_state(
        self: Arc<Self>,
        request_rx: Receiver<Request>,
        response_rx: Receiver<Response>,
    ) {
        let s1 = self.clone();
        let s2 = self.clone();

        self.task_handle.spawn(async move {
            let mut client = Client {};
            let stream = ClientStream {
                sender: s1.send_channel.clone(),
                receiver: response_rx,
            };
            client.run(stream, &s1.index).await;
            s1.finish().await;
        });

        self.task_handle.spawn(async move {
            let mut server = Server {};
            let stream = ServerStream {
                sender: s2.send_channel.clone(),
                receiver: request_rx,
            };
            server.run(stream, &s2.index).await;
            s2.finish().await;
        });
    }

    async fn run_command_handler(
        self: Arc<Self>,
        mut rx: Receiver<Command>,
        request_rx: Receiver<Request>,
        response_rx: Receiver<Response>,
    ) {
        let mut ws = Vec::new();

        let mut rxs = Some((request_rx, response_rx));

        while let Some(command) = rx.recv().await {
            match command {
                Command::AddWriter(w) => {
                    ws.push(w);

                    if let Some(rxs) = rxs.take() {
                        self.clone().create_state(rxs.0, rxs.1);
                    }
                }
                Command::SendMessage(m) => {
                    assert!(!ws.is_empty());

                    while !ws.is_empty() {
                        if ws[0].write(&m).await.is_ok() {
                            break;
                        }
                        ws.remove(0);
                    }

                    if ws.is_empty() {
                        self.finish().await;
                    }
                }
            }
        }
    }

    async fn run_message_receiver(&self, mut r: ObjectReader) {
        self.state.lock().await.receiver_count += 1;

        loop {
            match r.read().await {
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
                self.request_tx.send(rq).await.unwrap();
            }
            Message::Response(rs) => {
                self.response_tx.send(rs).await.unwrap();
            }
        }
    }

    async fn finish(&self) {
        self.task_handle.abort_all();

        let on_finish = self.state.lock().await.on_finish.take();
        if let Some(on_finish) = on_finish {
            on_finish.await
        }
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
