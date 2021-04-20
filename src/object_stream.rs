use serde::{Serialize};
use std::io;
use tokio::{
    net::TcpStream,
    io::{AsyncWriteExt, AsyncReadExt},
};

// XXX: Maybe the following link presents a better approach?
// https://stackoverflow.com/a/56533292/273348

/// Max object size when serialized in bytes.
const MAX_SERIALIZED_OBJECT_SIZE: usize = 512 * 1024; // 0.5MB

pub struct ObjectStream {
    byte_stream: TcpStream,
}

/// Like TcpStream, but allows writing and reading serializable/deserializable objects. Objects
/// are size delimited with maximum serialized size being MAX_SERIALIZED_OBJECT_SIZE.
impl ObjectStream {
    pub fn new(byte_stream: TcpStream) -> ObjectStream {
        ObjectStream {
            byte_stream,
        }
    }

    /// Write object into the stream
    pub async fn write<Object: ?Sized>(&mut self, object: &Object) -> io::Result<()>
    where
        Object: Serialize
    {
        // XXX: Can the Vec returned here be reused?
        let data = bincode::serialize(&object).unwrap();

        if data.len() > MAX_SERIALIZED_OBJECT_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "ObjectStream::write: object is too big"));
        }

        self.byte_stream.write_u32(data.len() as u32).await?;
        self.byte_stream.write_all(&data).await
    }

    /// Read object from the stream
    pub async fn read<Object>(&mut self) -> io::Result<Object>
    where
        Object: serde::de::DeserializeOwned
    {
        let len = self.byte_stream.read_u32().await? as usize;

        if len > MAX_SERIALIZED_OBJECT_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                "ObjectStream::read: received object would be too big"));
        }

        let mut buffer = vec![0; len];

        self.byte_stream.read_exact(&mut buffer).await?;

        bincode::deserialize::<Object>(&buffer)
            .map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData,
                    "ObjectStream::read: failed to deserialize object")
            })
    }
}
