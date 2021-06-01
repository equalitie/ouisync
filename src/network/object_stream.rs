use serde::Serialize;
use std::io;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
};

/// Max object size when serialized in bytes.
const MAX_SERIALIZED_OBJECT_SIZE: usize = 512 * 1024; // 0.5MB

/// Like TcpStream, but allows writing and reading serializable/deserializable objects. Objects
/// are size delimited with maximum serialized size being MAX_SERIALIZED_OBJECT_SIZE.
pub struct ObjectStream<T> {
    inner: T,
}

impl<T> ObjectStream<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T: AsyncRead + Unpin> ObjectStream<T> {
    ///
    /// Read object from the stream
    ///
    pub async fn read<Object>(&mut self) -> io::Result<Object>
    where
        Object: serde::de::DeserializeOwned,
    {
        let len = self.inner.read_u32().await? as usize;

        if len > MAX_SERIALIZED_OBJECT_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ObjectStream::read: received object would be too big",
            ));
        }

        let mut buffer = vec![0; len];

        self.inner.read_exact(&mut buffer).await?;

        bincode::deserialize(&buffer).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "ObjectStream::read: failed to deserialize object",
            )
        })
    }
}

impl<T: AsyncWrite + Unpin> ObjectStream<T> {
    ///
    /// Write object into the stream
    ///
    pub async fn write<Object: ?Sized>(&mut self, object: &Object) -> io::Result<()>
    where
        Object: Serialize,
    {
        // XXX: Can the Vec returned here be reused?
        let data = bincode::serialize(&object).unwrap();

        if data.len() > MAX_SERIALIZED_OBJECT_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ObjectStream::write: object is too big",
            ));
        }

        self.inner.write_u32(data.len() as u32).await?;
        self.inner.write_all(&data).await
    }
}

impl ObjectStream<TcpStream> {
    ///
    /// Split the stream into reader and writer to enable concurrent reading and writing.
    ///
    pub fn into_split(self) -> (ObjectStream<OwnedReadHalf>, ObjectStream<OwnedWriteHalf>) {
        let (r, w) = self.inner.into_split();
        (ObjectStream { inner: r }, ObjectStream { inner: w })
    }
}

pub type TcpObjectStream = ObjectStream<TcpStream>;
pub type TcpObjectReader = ObjectStream<OwnedReadHalf>;
pub type TcpObjectWriter = ObjectStream<OwnedWriteHalf>;
