use crate::async_object::{AbortHandles, AsyncObject, AsyncObjectTrait};
use crate::client::Client;
use crate::message::{Message, Request, Response};
use crate::object_stream::{ObjectReader, ObjectStream, ObjectWriter};
use crate::server::Server;
use std::ops::FnOnce;
use std::sync::Arc;
use tokio::sync::mpsc::{error::SendError, Receiver, Sender};

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

type OnFinish = Box<dyn FnOnce() + Send>;

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
    send_channel: Sender<Command>,
    request_tx: Sender<Request>,
    response_tx: Sender<Response>,
    state: std::sync::Mutex<State>,
    abort_handles: AbortHandles,
}

impl MessageBroker {
    pub fn new(on_finish: OnFinish) -> AsyncObject<MessageBroker> {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(1);
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(1);
        let (response_tx, response_rx) = tokio::sync::mpsc::channel(1);

        let broker = Arc::new(MessageBroker {
            send_channel: command_tx,
            request_tx,
            response_tx,
            state: std::sync::Mutex::new(State {
                on_finish: Some(on_finish),
                receiver_count: 0,
            }),
            abort_handles: AbortHandles::new(),
        });

        let b = broker.clone();

        broker.abortable_spawn(Self::run_command_handler(
            b,
            command_rx,
            request_rx,
            response_rx,
        ));

        AsyncObject::new(broker)
    }

    fn create_state(
        self: Arc<Self>,
        request_rx: Receiver<Request>,
        response_rx: Receiver<Response>,
    ) {
        let s1 = self.clone();
        let s2 = self.clone();

        self.abortable_spawn(async move {
            let mut client = Client {};
            let stream = ClientStream {
                sender: s1.send_channel.clone(),
                receiver: response_rx,
            };
            client.run(stream).await;
            s1.finish();
        });

        self.abortable_spawn(async move {
            let mut server = Server {};
            let stream = ServerStream {
                sender: s2.send_channel.clone(),
                receiver: request_rx,
            };
            server.run(stream).await;
            s2.finish();
        });
    }

    pub fn add_connection(self: Arc<Self>, con: ObjectStream) {
        let s = self.clone();
        let send_channel = self.send_channel.clone();

        self.abortable_spawn(async move {
            let (r, w) = con.into_split();

            if send_channel.send(Command::AddWriter(w)).await.is_err() {
                println!("Failed to activate writer");
                // XXX: Tell self to abort
                return;
            }

            s.run_message_receiver(r).await;
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
                        self.finish();
                    }
                }
            }
        }
    }

    async fn run_message_receiver(self: Arc<Self>, mut r: ObjectReader) {
        self.state.lock().unwrap().receiver_count += 1;
        loop {
            match r.read::<Message>().await {
                Ok(msg) => {
                    self.handle_message(msg).await;
                }
                Err(_) => {
                    let rc = {
                        let mut state = self.state.lock().unwrap();
                        state.receiver_count -= 1;
                        state.receiver_count
                    };

                    if rc == 0 {
                        self.finish();
                    }
                    break;
                }
            };
        }
    }

    async fn handle_message(self: &Arc<Self>, msg: Message) {
        match msg {
            Message::Request(rq) => {
                self.request_tx.send(rq).await.unwrap();
            }
            Message::Response(rs) => {
                self.response_tx.send(rs).await.unwrap();
            }
        }
    }

    fn finish(&self) {
        self.abort();
        if let Some(on_finish) = self.state.lock().unwrap().on_finish.take() {
            on_finish();
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

impl AsyncObjectTrait for MessageBroker {
    fn abort_handles(&self) -> &AbortHandles {
        &self.abort_handles
    }
}
