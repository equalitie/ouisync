use super::message::{Header, Message};
use futures_util::{ready, Sink, Stream};
use std::{
    io, mem,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Max message size when serialized in bytes.
/// This is also the maximum allowed message size in the Noise Protocol Framework.
const MAX_MESSAGE_SIZE: u16 = u16::MAX - 1;

// Messages are encoded like this:
//
// [ header: `Header::SIZE` bytes ][ len: 2 bytes ][ content: `len` bytes ]
//

/// Wrapper that turns a reader (`AsyncRead`) into a `Stream` of `Message`.
pub(crate) struct MessageStream<R> {
    read: R,
    decoder: Decoder,
}

impl<R> MessageStream<R> {
    pub fn new(read: R) -> Self {
        Self {
            read,
            decoder: Decoder::default(),
        }
    }
}

impl<R> Stream for MessageStream<R>
where
    R: AsyncRead + Unpin,
{
    type Item = io::Result<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let mut read = Pin::new(&mut this.read);
        this.decoder.poll_next(read.as_mut(), cx).map(Some)
    }
}

/// Wrapper that turns a writer (`AsyncWrite`) into a `Sink` of `Message`.
pub(crate) struct MessageSink<W> {
    write: W,
    encoder: Encoder,
}

impl<W> MessageSink<W> {
    pub fn new(write: W) -> Self {
        Self {
            write,
            encoder: Encoder::default(),
        }
    }
}

impl<W> Sink<Message> for MessageSink<W>
where
    W: AsyncWrite + Unpin,
{
    type Error = io::Error;

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.encoder.start(item)
    }

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        let this = &mut *self;
        let write = Pin::new(&mut this.write);
        this.encoder.poll_ready(write, cx)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_ready(cx))?;

        match &self.encoder.state {
            EncodeState::Idle => Poll::Ready(Ok(())),
            EncodeState::Sending {
                phase: SendingPhase::Done,
                ..
            } => {
                let result = ready!(Pin::new(&mut self.write).poll_flush(cx));
                self.encoder.state = EncodeState::Idle;
                Poll::Ready(result)
            }
            EncodeState::Sending { .. } => unreachable!(),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_ready(cx))?;

        match &self.encoder.state {
            EncodeState::Idle
            | EncodeState::Sending {
                phase: SendingPhase::Done,
                ..
            } => {
                let result = ready!(Pin::new(&mut self.write).poll_shutdown(cx));
                self.encoder.state = EncodeState::Idle;
                Poll::Ready(result)
            }
            EncodeState::Sending { .. } => unreachable!(),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Encoder

struct Encoder {
    state: EncodeState,
    offset: usize,
}

enum EncodeState {
    Idle,
    Sending {
        message: Message,
        phase: SendingPhase,
    },
}

enum SendingPhase {
    Header,
    Len,
    Content,
    Done,
}

impl Default for Encoder {
    fn default() -> Self {
        Self {
            state: EncodeState::Idle,
            offset: 0,
        }
    }
}

impl Encoder {
    fn is_sending(&self) -> bool {
        match &self.state {
            EncodeState::Idle => false,
            EncodeState::Sending {
                phase: SendingPhase::Done,
                ..
            } => false,
            EncodeState::Sending { .. } => true,
        }
    }

    fn start(&mut self, message: Message) -> Result<(), io::Error> {
        assert!(
            !self.is_sending(),
            "start_send called while already sending"
        );

        if message.content.len() > MAX_MESSAGE_SIZE as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, LengthError));
        }

        self.state = EncodeState::Sending {
            message,
            phase: SendingPhase::Header,
        };
        self.offset = 0;

        Ok(())
    }

    fn poll_ready<W>(
        &mut self,
        mut io: Pin<&mut W>,
        cx: &mut Context,
    ) -> Poll<Result<(), io::Error>>
    where
        W: AsyncWrite,
    {
        loop {
            match &mut self.state {
                EncodeState::Idle => return Poll::Ready(Ok(())),
                EncodeState::Sending { message, phase } => match phase {
                    SendingPhase::Header => {
                        match ready!(poll_write_all(
                            io.as_mut(),
                            cx,
                            &message.header().serialize(),
                            &mut self.offset
                        )) {
                            Ok(true) => {
                                *phase = SendingPhase::Len;
                                self.offset = 0;
                            }
                            Ok(false) => (),
                            Err(error) => {
                                self.state = EncodeState::Idle;
                                return Poll::Ready(Err(error));
                            }
                        }
                    }
                    SendingPhase::Len => {
                        let buffer = (message.content.len() as u16).to_be_bytes();

                        match ready!(poll_write_all(io.as_mut(), cx, &buffer, &mut self.offset)) {
                            Ok(true) => {
                                if message.content.is_empty() {
                                    *phase = SendingPhase::Done;
                                } else {
                                    *phase = SendingPhase::Content;
                                }
                                self.offset = 0;
                            }
                            Ok(false) => (),
                            Err(error) => {
                                self.state = EncodeState::Idle;
                                return Poll::Ready(Err(error));
                            }
                        }
                    }
                    SendingPhase::Content => {
                        match ready!(poll_write_all(
                            io.as_mut(),
                            cx,
                            &message.content,
                            &mut self.offset
                        )) {
                            Ok(true) => {
                                *phase = SendingPhase::Done;
                                self.offset = 0;
                            }
                            Ok(false) => (),
                            Err(error) => {
                                self.state = EncodeState::Idle;
                                return Poll::Ready(Err(error));
                            }
                        }
                    }
                    SendingPhase::Done => return Poll::Ready(Ok(())),
                },
            }
        }
    }
}

fn poll_write_all<W>(
    io: Pin<&mut W>,
    cx: &mut Context,
    buffer: &[u8],
    offset: &mut usize,
) -> Poll<io::Result<bool>>
where
    W: AsyncWrite,
{
    let len = ready!(io.poll_write(cx, &buffer[*offset..]))?;

    if len == 0 {
        return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
    }

    *offset += len;

    Poll::Ready(Ok(*offset >= buffer.len()))
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Decoder

struct Decoder {
    phase: DecodePhase,
    buffer: Vec<u8>,
    offset: usize,
}

#[derive(Clone, Copy)]
enum DecodePhase {
    Header,
    Len { header: Header },
    Content { header: Header },
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            phase: DecodePhase::Header,
            buffer: vec![0; Header::SIZE],
            offset: 0,
        }
    }
}

impl Decoder {
    fn poll_next<R>(&mut self, mut io: Pin<&mut R>, cx: &mut Context) -> Poll<io::Result<Message>>
    where
        R: AsyncRead,
    {
        loop {
            ready!(self.poll_read_exact(io.as_mut(), cx))?;

            match self.phase {
                DecodePhase::Header => {
                    let header: [u8; Header::SIZE] = self
                        .filled()
                        .try_into()
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

                    let header = match Header::deserialize(&header) {
                        Some(header) => header,
                        None => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                BadHeader,
                            )))
                        }
                    };

                    self.phase = DecodePhase::Len { header };
                    self.buffer.resize(2, 0);
                    self.offset = 0;
                }
                DecodePhase::Len { header } => {
                    let len = u16::from_be_bytes(
                        self.filled()
                            .try_into()
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
                    );

                    if len > MAX_MESSAGE_SIZE {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            LengthError,
                        )));
                    }

                    if len > 0 {
                        self.phase = DecodePhase::Content { header };
                        self.buffer.resize(len as usize, 0);
                        self.offset = 0;
                    } else {
                        self.phase = DecodePhase::Header;
                        self.buffer.resize(Header::SIZE, 0);
                        self.offset = 0;

                        return Poll::Ready(Ok((header, Vec::new()).into()));
                    }
                }
                DecodePhase::Content { header } => {
                    let content = mem::take(&mut self.buffer);

                    self.phase = DecodePhase::Header;
                    self.buffer.resize(Header::SIZE, 0);
                    self.offset = 0;

                    return Poll::Ready(Ok((header, content).into()));
                }
            }
        }
    }

    fn poll_read_exact<R>(&mut self, mut io: Pin<&mut R>, cx: &mut Context) -> Poll<io::Result<()>>
    where
        R: AsyncRead,
    {
        loop {
            let mut buf = ReadBuf::new(&mut self.buffer[self.offset..]);

            match ready!(io.as_mut().poll_read(cx, &mut buf)) {
                Ok(()) if !buf.filled().is_empty() => {
                    self.offset += buf.filled().len();

                    if self.offset >= self.buffer.len() {
                        return Poll::Ready(Ok(()));
                    }
                }
                Ok(()) => return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into())),
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }

    fn filled(&self) -> &[u8] {
        &self.buffer[..self.offset]
    }
}

#[derive(Debug, Error)]
#[error("message too big")]
struct LengthError;

#[derive(Debug, Error)]
#[error("bad header")]
struct BadHeader;
