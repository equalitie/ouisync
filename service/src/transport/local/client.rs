use futures_util::{SinkExt, StreamExt};

use crate::{
    protocol::{Message, MessageId, Request, Response, ResponseResult, UnexpectedResponse},
    transport::ClientError,
};

use super::{LocalClientReader, LocalClientWriter, LocalEndpoint};

pub struct LocalClient {
    reader: LocalClientReader,
    writer: LocalClientWriter,
}

impl LocalClient {
    pub async fn connect(endpoint: &LocalEndpoint) -> Result<Self, ClientError> {
        let (reader, writer) = super::connect(endpoint).await?;
        Ok(Self { reader, writer })
    }

    /// # Cancel safety
    ///
    /// This method is currently *not cancel safe*. If it's cancelled, the request might have
    /// already been sent and so next time this method is invoked, it might receive the response
    /// from the previous invocation which would cause it to return
    /// `ClientError::UnexpectedResponse` and the response would be lost.
    ///
    /// TODO: make it cancel safe.
    pub async fn invoke<T>(&mut self, request: Request) -> Result<T, ClientError>
    where
        T: TryFrom<Response, Error = UnexpectedResponse>,
    {
        let id = MessageId::next();

        self.writer
            .send(Message {
                id,
                payload: request,
            })
            .await?;

        let message = match self.reader.next().await {
            Some(Ok(message)) if message.id == id => message,
            Some(Ok(_)) => return Err(ClientError::UnexpectedResponse),
            Some(Err(error)) => return Err(error.into()),
            None => return Err(ClientError::Disconnected),
        };

        match message.payload {
            ResponseResult::Success(response) => Ok(response.try_into()?),
            ResponseResult::Failure(error) => Err(error.into()),
        }
    }

    pub async fn close(&mut self) -> Result<(), ClientError> {
        self.writer.close().await?;
        Ok(())
    }
}
