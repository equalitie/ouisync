use std::time::Instant;

use futures_util::{stream::FuturesUnordered, SinkExt};
use ouisync::NetworkEventReceiver;
use tokio::select;
use tokio_stream::{StreamExt, StreamMap};
use tracing::{field, Span};

use crate::{
    protocol::{DecodeError, Message, MessageId, ProtocolError, Request, Response, ResponseResult},
    state::{RepositorySubscription, StateMonitorSubscription, Unsubscribe},
    subscription::SubscriptionStream,
    transport::{ReadError, ServerReader, ServerWriter},
    State,
};

/// Connection to a client.
pub(crate) struct Connection {
    reader: ServerReader,
    writer: ServerWriter,
    subscriptions: StreamMap<MessageId, SubscriptionStream>,
}

impl Connection {
    pub fn new(reader: ServerReader, writer: ServerWriter) -> Self {
        Self {
            reader,
            writer,
            subscriptions: StreamMap::new(),
        }
    }

    pub async fn run(&mut self, state: &State) {
        let mut handlers = FuturesUnordered::new();

        loop {
            let result = select! {
                // request received
                result = self.reader.next() => {
                    match self.preprocess_request(result).await {
                        Ok(Some(message)) => {
                            handlers.push(handle_request(state, message));
                            Ok(())
                        }
                        Ok(None) => Ok(()),
                        Err(error) => Err(error),
                    }
                }
                // event emitted
                Some((id, payload)) = self.subscriptions.next() => {
                    self.send_response(Message {
                        id,
                        payload: ResponseResult::Success(payload),
                    }).await
                }
                // request handler completed
                Some((id, action, span)) = handlers.next() => {
                    self.handle_action(id, action, span).await
                }
            };

            match result {
                Ok(()) => (),
                Err(Terminate) => break,
            }
        }
    }

    pub async fn close(&mut self) {
        if let Err(error) = self.writer.close().await {
            tracing::error!(?error, "failed to close connection");
        }
    }

    async fn preprocess_request(
        &mut self,
        message: Option<Result<Message<Request>, ReadError>>,
    ) -> Result<Option<Message<Request>>, Terminate> {
        match message {
            Some(Ok(message)) => Ok(Some(message)),
            Some(Err(ReadError::Receive(error))) => {
                tracing::error!(?error, "failed to receive message");
                Err(Terminate)
            }
            Some(Err(ReadError::Decode(DecodeError::Id))) => {
                tracing::error!("failed to decode message id");
                Err(Terminate)
            }
            Some(Err(ReadError::Decode(DecodeError::Payload(id, error)))) => {
                tracing::warn!(?error, ?id, "failed to decode message payload");

                self.send_response(Message {
                    id,
                    payload: ResponseResult::Failure(error.into()),
                })
                .await?;

                Ok(None)
            }
            Some(Err(ReadError::Validate(id, error))) => {
                tracing::warn!(?error, ?id, "failed to validate message");

                self.send_response(Message {
                    id,
                    payload: ResponseResult::Failure(error.into()),
                })
                .await?;

                Ok(None)
            }
            None => Err(Terminate),
        }
    }

    async fn handle_action(
        &mut self,
        id: MessageId,
        action: Action<ResponseResult>,
        span: Span,
    ) -> Result<(), Terminate> {
        let payload = match action {
            Action::Reply(payload) => payload,
            Action::Subscribe(subscription) => {
                self.subscriptions.insert(id, subscription);
                ResponseResult::Success(Response::None)
            }
            Action::Unsubscribe(id) => {
                self.subscriptions.remove(&id);
                ResponseResult::Success(Response::None)
            }
        };

        span.in_scope(|| tracing::trace!(response = ?payload));

        self.send_response(Message { id, payload }).await
    }

    async fn send_response(&mut self, message: Message<ResponseResult>) -> Result<(), Terminate> {
        match self.writer.send(message).await {
            Ok(()) => Ok(()),
            Err(error) => {
                tracing::error!(?error, "failed to send message");
                Err(Terminate)
            }
        }
    }
}

async fn handle_request(
    state: &State,
    message: Message<Request>,
) -> (MessageId, Action<ResponseResult>, Span) {
    let span = tracing::trace_span!("request", message = ?message.payload, elapsed = field::Empty);
    let start = Instant::now();

    let action = match dispatch(state, message.payload).await {
        Ok(Action::Reply(response)) => Action::Reply(ResponseResult::Success(response)),
        Ok(Action::Subscribe(sub)) => Action::Subscribe(sub),
        Ok(Action::Unsubscribe(id)) => Action::Unsubscribe(id),
        Err(error) => Action::Reply(ResponseResult::Failure(error)),
    };

    span.record("elapsed", field::debug(start.elapsed()));

    (message.id, action, span)
}

struct Terminate;

enum Action<T> {
    Reply(T),
    Subscribe(SubscriptionStream),
    Unsubscribe(MessageId),
}

impl<T> From<T> for Action<Response>
where
    T: Into<Response>,
{
    fn from(response: T) -> Self {
        Action::Reply(response.into())
    }
}

impl From<RepositorySubscription> for Action<Response> {
    fn from(rx: RepositorySubscription) -> Self {
        Self::Subscribe(rx.into())
    }
}

impl From<NetworkEventReceiver> for Action<Response> {
    fn from(rx: NetworkEventReceiver) -> Self {
        Self::Subscribe(rx.into())
    }
}

impl From<StateMonitorSubscription> for Action<Response> {
    fn from(rx: StateMonitorSubscription) -> Self {
        Self::Subscribe(rx.into())
    }
}

impl From<Unsubscribe> for Action<Response> {
    fn from(unsub: Unsubscribe) -> Self {
        Self::Unsubscribe(unsub.0)
    }
}

// The `dispatch` function is auto-generated with `build.rs` from the `#[api]` annotated
// methods in `impl State`.
include!(concat!(env!("OUT_DIR"), "/connection.rs"));
