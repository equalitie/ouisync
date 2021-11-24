use futures_util::{Sink, Stream};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Max object size when serialized in bytes.
const MAX_SERIALIZED_OBJECT_SIZE: u16 = u16::MAX - 1;

/// Wrapper that turns a reader (`AsyncRead`) into a `Stream` of `T` by deserializing the data read
/// from the reader.
pub(crate) struct ObjectRead<T, R> {
    read: R,
    decoder: Decoder,
    _ty: PhantomData<fn() -> T>,
}

impl<T, R> ObjectRead<T, R> {
    pub fn new(read: R) -> Self {
        Self {
            read,
            decoder: Decoder::default(),
            _ty: PhantomData,
        }
    }
}

impl<T, R> Stream for ObjectRead<T, R>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    type Item = io::Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let mut read = Pin::new(&mut this.read);

        loop {
            match this.decoder.phase {
                DecodePhase::Len => {
                    match poll_read_exact(read.as_mut(), cx, &mut this.decoder.buffer) {
                        Poll::Ready(Ok(())) => {
                            let len = u16::from_be_bytes(
                                this.decoder.buffer.filled().try_into().unwrap(),
                            );

                            if len > MAX_SERIALIZED_OBJECT_SIZE {
                                return Poll::Ready(Some(Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    LengthError,
                                ))));
                            }

                            this.decoder.buffer.reset(len as usize);
                            this.decoder.phase = DecodePhase::Data;
                        }
                        Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                DecodePhase::Data => {
                    match poll_read_exact(read.as_mut(), cx, &mut this.decoder.buffer) {
                        Poll::Ready(Ok(())) => {
                            let result = bincode::deserialize(this.decoder.buffer.filled())
                                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
                            this.decoder.buffer.reset(2);
                            this.decoder.phase = DecodePhase::Len;

                            return Poll::Ready(Some(result));
                        }
                        Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                        Poll::Pending => return Poll::Pending,
                    }
                }
            }
        }
    }
}

/// Wrapper that turns a writer (`AsyncWrite`) into a `Sink` of `T` by serializing the items and
/// writing them to the writer.
pub(crate) struct ObjectWrite<T, W> {
    write: W,
    encoder: Encoder,
    _ty: PhantomData<fn() -> T>,
}

impl<T, W> ObjectWrite<T, W> {
    pub fn new(write: W) -> Self {
        Self {
            write,
            encoder: Encoder::default(),
            _ty: PhantomData,
        }
    }
}

impl<'a, T, W> Sink<&'a T> for ObjectWrite<T, W>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    type Error = io::Error;

    fn start_send(mut self: Pin<&mut Self>, item: &'a T) -> Result<(), Self::Error> {
        assert!(
            matches!(self.encoder.phase, EncodePhase::Ready),
            "start_send called while not ready"
        );

        let data = bincode::serialize(item)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        if data.len() > MAX_SERIALIZED_OBJECT_SIZE as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, LengthError));
        }

        self.encoder.phase = EncodePhase::Len { offset: 0 };
        self.encoder.buffer = Buffer::new(data);

        Ok(())
    }

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        let this = &mut *self;
        let mut write = Pin::new(&mut this.write);

        loop {
            match this.encoder.phase {
                EncodePhase::Ready => return Poll::Ready(Ok(())),
                EncodePhase::Len { offset } => {
                    let buffer = (this.encoder.buffer.remaining().len() as u16).to_be_bytes();
                    let buffer = &buffer[offset..];

                    match write.as_mut().poll_write(cx, buffer) {
                        Poll::Ready(Ok(len)) if len > 0 => {
                            if offset + len >= 2 {
                                this.encoder.phase = EncodePhase::Data;
                            } else {
                                this.encoder.phase = EncodePhase::Len {
                                    offset: offset + len,
                                };
                            }
                        }
                        Poll::Ready(Ok(_)) => {
                            return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
                        }
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                EncodePhase::Data => {
                    match write
                        .as_mut()
                        .poll_write(cx, this.encoder.buffer.remaining())
                    {
                        Poll::Ready(Ok(len)) if len > 0 => {
                            this.encoder.buffer.advance(len);

                            if this.encoder.buffer.remaining().is_empty() {
                                this.encoder.phase = EncodePhase::Ready;
                                this.encoder.buffer.reset(0);

                                return Poll::Ready(Ok(()));
                            }
                        }
                        Poll::Ready(Ok(_)) => {
                            return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
                        }
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.as_mut().poll_ready(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.write).poll_flush(cx),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        match self.as_mut().poll_ready(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.write).poll_shutdown(cx),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

struct Encoder {
    phase: EncodePhase,
    buffer: Buffer,
}

impl Default for Encoder {
    fn default() -> Self {
        Self {
            phase: EncodePhase::Ready,
            buffer: Buffer::new(vec![]),
        }
    }
}

enum EncodePhase {
    Ready,
    Len { offset: usize },
    Data,
}

struct Decoder {
    phase: DecodePhase,
    buffer: Buffer,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            phase: DecodePhase::Len,
            buffer: Buffer::new(vec![0; 2]),
        }
    }
}

#[derive(Clone, Copy)]
enum DecodePhase {
    Len,
    Data,
}

struct Buffer {
    data: Vec<u8>,
    offset: usize,
}

impl Buffer {
    fn new(data: Vec<u8>) -> Self {
        Self { data, offset: 0 }
    }

    fn reset(&mut self, size: usize) {
        self.data.resize(size, 0);
        self.offset = 0;
    }

    fn as_read_buf(&mut self) -> ReadBuf {
        ReadBuf::new(&mut self.data[self.offset..])
    }

    fn filled(&self) -> &[u8] {
        &self.data[0..self.offset]
    }

    fn remaining(&self) -> &[u8] {
        &self.data[self.offset..]
    }

    fn advance(&mut self, len: usize) {
        self.offset += len;
    }
}

fn poll_read_exact<S>(
    mut io: Pin<&mut S>,
    cx: &mut Context,
    buffer: &mut Buffer,
) -> Poll<io::Result<()>>
where
    S: AsyncRead,
{
    loop {
        let mut buf = buffer.as_read_buf();

        match io.as_mut().poll_read(cx, &mut buf) {
            Poll::Ready(Ok(())) if !buf.filled().is_empty() => {
                let filled = buf.filled().len();
                buffer.advance(filled);

                if buffer.remaining().is_empty() {
                    return Poll::Ready(Ok(()));
                }
            }
            Poll::Ready(Ok(())) => return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into())),
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => return Poll::Pending,
        }
    }
}

#[derive(Debug, Error)]
#[error("serialized object size too big")]
struct LengthError;
