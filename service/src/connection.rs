use std::{pin::Pin, time::Instant};

use futures_util::SinkExt;
use ouisync::{DhtLookupStream, NetworkEventReceiver};
use tokio::select;
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use tracing::{Span, field};

use crate::{
    State,
    futures_map::FuturesMap,
    protocol::{DecodeError, Message, MessageId, ProtocolError, Request, Response, ResponseResult},
    state::{RepositorySubscription, StateMonitorSubscription},
    subscription::SubscriptionStream,
    transport::{ReadError, ServerReader, ServerWriter},
};

/// Connection to a client.
pub(crate) struct Connection<'state> {
    reader: ServerReader,
    writer: ServerWriter,
    handlers: FuturesMap<MessageId, Pin<Box<Handler<'state>>>>,
    subscriptions: StreamMap<MessageId, StreamNotifyClose<SubscriptionStream>>,
}

type Handler<'state> = dyn Future<Output = (Action<ResponseResult>, Span)> + Send + 'state;

impl<'state> Connection<'state> {
    pub fn new(reader: ServerReader, writer: ServerWriter) -> Self {
        Self {
            reader,
            writer,
            handlers: FuturesMap::new(),
            subscriptions: StreamMap::new(),
        }
    }

    pub async fn run(&mut self, state: &'state State) {
        loop {
            let result = select! {
                // request received
                result = self.reader.next() => {
                    self.handle_request(state, result).await
                }
                // event emitted
                Some((id, payload)) = self.subscriptions.next() => {
                    self.handle_event(id, payload).await
                }
                // request handler completed
                Some((id, (action, span))) = self.handlers.next() => {
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

    async fn handle_request(
        &mut self,
        state: &'state State,
        request: Option<Result<Message<Request>, ReadError>>,
    ) -> Result<(), Terminate> {
        match self.preprocess_request(request).await {
            Ok(Some(message)) => {
                if let Request::Cancel { id } = &message.payload {
                    // Cancel existing request or subscription
                    let handler_removed = self.handlers.remove(id);
                    let subscription_removed = self.subscriptions.remove(id).is_some();

                    self.send_response(Message {
                        id: message.id,
                        payload: ResponseResult::Success(Response::Bool(
                            handler_removed || subscription_removed,
                        )),
                    })
                    .await?;
                } else {
                    // Invoke request handler
                    if self.handlers.insert(
                        message.id,
                        Box::pin(dispatch_request(state, message.payload)),
                    ) {
                        tracing::warn!(message_id = ?message.id, "message id not unique");
                    }
                }

                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn handle_event(
        &mut self,
        id: MessageId,
        payload: Option<Response>,
    ) -> Result<(), Terminate> {
        self.send_response(Message {
            id,
            payload: ResponseResult::Success(payload.unwrap_or(Response::None)),
        })
        .await
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
                self.subscriptions
                    .insert(id, StreamNotifyClose::new(subscription));
                ResponseResult::Success(Response::Unit)
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

async fn dispatch_request(state: &State, request: Request) -> (Action<ResponseResult>, Span) {
    let span = tracing::trace_span!("request", message = ?request, elapsed = field::Empty);
    let start = Instant::now();

    let action = match dispatch(state, request).await {
        Ok(Action::Reply(response)) => Action::Reply(ResponseResult::Success(response)),
        Ok(Action::Subscribe(sub)) => Action::Subscribe(sub),
        Err(error) => Action::Reply(ResponseResult::Failure(error)),
    };

    span.record("elapsed", field::debug(start.elapsed()));

    (action, span)
}

struct Terminate;

enum Action<T> {
    Reply(T),
    Subscribe(SubscriptionStream),
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
    fn from(stream: RepositorySubscription) -> Self {
        Self::Subscribe(stream.into())
    }
}

impl From<NetworkEventReceiver> for Action<Response> {
    fn from(stream: NetworkEventReceiver) -> Self {
        Self::Subscribe(stream.into())
    }
}

impl From<StateMonitorSubscription> for Action<Response> {
    fn from(stream: StateMonitorSubscription) -> Self {
        Self::Subscribe(stream.into())
    }
}

impl From<DhtLookupStream> for Action<Response> {
    fn from(stream: DhtLookupStream) -> Self {
        Self::Subscribe(stream.into())
    }
}

// The `dispatch` function is auto-generated with `build.rs` from the `#[api]` annotated
// methods in `impl State`.
include!(concat!(env!("OUT_DIR"), "/connection.rs"));
