use std::{
    io::{self, Cursor},
    time::Duration,
};

use bytes::{Buf as _, BytesMut};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, BufWriter},
    net::TcpStream,
    time::error::Elapsed,
};

use crate::message::{FrameError, Message, MessageError, MessageFrames};

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("read timeout: {0}")]
    ReadTimeout(Elapsed),
    #[error("read message frames: {source}")]
    ReadMessageFrames { source: MessageError },
    #[error("parse message frames: {source}")]
    ParseMessageFrames { source: FrameError },
    #[error("read message io: {source}")]
    ReadMessageIo { source: io::Error },
    #[error("connection reset by peer")]
    ResetByPeer,
}

impl ConnectionError {
    fn read_timeout(elapsed: Elapsed) -> Self {
        ConnectionError::ReadTimeout(elapsed)
    }

    fn read_message_frames(source: MessageError) -> Self {
        ConnectionError::ReadMessageFrames { source }
    }

    fn parse_message_frames(source: FrameError) -> Self {
        ConnectionError::ParseMessageFrames { source }
    }

    fn read_message_io(source: io::Error) -> Self {
        ConnectionError::ReadMessageIo { source }
    }
}

pub struct Connection<Stream = TcpStream> {
    stream: BufWriter<Stream>,
    // The buffer for reading frames.
    buffer: BytesMut,
}

impl<Stream> Connection<Stream>
where
    Stream: AsyncWrite,
{
    pub fn new(stream: Stream, buffer_size: usize) -> Self {
        Self {
            stream: BufWriter::new(stream),
            buffer: BytesMut::with_capacity(buffer_size),
        }
    }
}

impl<Stream> Connection<Stream>
where
    Stream: AsyncRead + Unpin,
    BufWriter<Stream>: AsyncRead,
{
    pub async fn read_message_with_timeout(
        &mut self,
        duration: Duration,
    ) -> Result<Option<Message>, ConnectionError> {
        match tokio::time::timeout(duration, self.read_message()).await {
            Ok(read_result) => read_result,
            Err(elapsed) => Err(ConnectionError::read_timeout(elapsed)),
        }
    }

    pub async fn read_message(&mut self) -> Result<Option<Message>, ConnectionError> {
        match self.read_message_frames().await? {
            Some(message_frames) => Ok(Some(
                Message::parse(message_frames).map_err(ConnectionError::read_message_frames)?,
            )),
            None => Ok(None),
        }
    }

    async fn read_message_frames(&mut self) -> Result<Option<MessageFrames>, ConnectionError> {
        loop {
            if let Some(message_frames) = self.parse_message_frames()? {
                return Ok(Some(message_frames));
            }

            if 0 == self
                .stream
                .read_buf(&mut self.buffer)
                .await
                .map_err(ConnectionError::read_message_io)?
            {
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    Err(ConnectionError::ResetByPeer)
                };
            }
        }
    }

    fn parse_message_frames(&mut self) -> Result<Option<MessageFrames>, ConnectionError> {
        use FrameError::Incomplete;

        let mut buf = Cursor::new(&self.buffer[..]);

        match MessageFrames::check_parse(&mut buf) {
            Ok(()) => {
                #[allow(clippy::cast_possible_truncation)]
                let len = buf.position() as usize;
                buf.set_position(0);
                let message_frames = MessageFrames::parse(&mut buf)
                    .map_err(ConnectionError::parse_message_frames)?;
                self.buffer.advance(len);

                Ok(Some(message_frames))
            }
            Err(Incomplete) => Ok(None),
            // TODO: define distinct error
            Err(e) => Err(ConnectionError::parse_message_frames(e)),
        }
    }
}