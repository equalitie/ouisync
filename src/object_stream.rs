use serde::Serialize;
use std::io;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
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
        ObjectStream { byte_stream }
    }

    ///
    /// Read object from the stream
    ///
    pub async fn read<Object>(&mut self) -> io::Result<Object>
    where
        Object: serde::de::DeserializeOwned,
    {
        read_impl(&mut self.byte_stream).await
    }

    ///
    /// Write object into the stream
    ///
    pub async fn write<Object: ?Sized>(&mut self, object: &Object) -> io::Result<()>
    where
        Object: Serialize,
    {
        write_impl(&mut self.byte_stream, &object).await
    }

    ///
    /// Split the stream into reader and writer to enable concurrent reading and writing.
    ///
    pub fn into_split(self) -> (ObjectReader, ObjectWriter) {
        let (r, w) = self.byte_stream.into_split();

        (
            ObjectReader { byte_reader: r },
            ObjectWriter { byte_writer: w },
        )
    }
}

///
/// Like ObjectStream, but only for reading
///
pub struct ObjectReader {
    byte_reader: OwnedReadHalf,
}

impl ObjectReader {
    /// Read object from the stream
    pub async fn read<Object>(&mut self) -> io::Result<Object>
    where
        Object: serde::de::DeserializeOwned,
    {
        read_impl(&mut self.byte_reader).await
    }
}

///
/// Like ObjectStream, but only for writing
///
pub struct ObjectWriter {
    byte_writer: OwnedWriteHalf,
}

impl ObjectWriter {
    /// Write object into the stream
    pub async fn write<Object: ?Sized>(&mut self, object: &Object) -> io::Result<()>
    where
        Object: Serialize,
    {
        write_impl(&mut self.byte_writer, &object).await
    }
}

///
/// Read object from the stream
///
async fn read_impl<R, Object>(byte_reader: &mut R) -> io::Result<Object>
where
    R: AsyncRead + Unpin,
    Object: serde::de::DeserializeOwned,
{
    let len = byte_reader.read_u32().await? as usize;

    if len > MAX_SERIALIZED_OBJECT_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ObjectStream::read: received object would be too big",
        ));
    }

    let mut buffer = vec![0; len];

    byte_reader.read_exact(&mut buffer).await?;

    bincode::deserialize::<Object>(&buffer).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "ObjectStream::read: failed to deserialize object",
        )
    })
}

///
/// Write object into the stream
///
async fn write_impl<W, Object: ?Sized>(
    byte_writer: &mut W,
    object: &Object,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
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

    byte_writer.write_u32(data.len() as u32).await?;
    byte_writer.write_all(&data).await
}
