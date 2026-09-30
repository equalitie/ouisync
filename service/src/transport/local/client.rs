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
    pub async fn connect(endpoint: LocalEndpoint) -> Result<Self, ClientError> {
        let (reader, writer) = super::connect(endpoint).await?;
        Ok(Self { reader, writer })
    }

    pub async fn invoke<T>(&mut self, request: Request) -> Result<T, ClientError>
    where
        T: TryFrom<Response, Error = UnexpectedResponse>,
    {
        self.writer
            .send(Message {
                id: MessageId::next(),
                payload: request,
            })
            .await?;

        let message = match self.reader.next().await {
            Some(Ok(message)) => message,
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
