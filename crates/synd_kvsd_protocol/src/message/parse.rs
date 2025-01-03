use std::string::FromUtf8Error;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::message::{frame::prefix, Authenticate, Message, MessageError, MessageType, Ping};

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("end of stream")]
    EndOfStream,
    #[error("invalid message type: {0}")]
    InvalidMessageType(#[from] MessageError),
    #[error("invalid utf8: {0}")]
    InvalidUtf8(#[from] FromUtf8Error),
    #[error("expect frame: {0}")]
    Expect(&'static str),
    #[error("incomplete")]
    Incomplete,
}

impl ParseError {
    #[expect(clippy::needless_pass_by_value)]
    fn expect(err: nom::Err<nom::error::Error<&[u8]>>, frame: &'static str) -> Self {
        if err.is_incomplete() {
            ParseError::Incomplete
        } else {
            ParseError::Expect(frame)
        }
    }
}

pub(crate) struct Parser;

impl Parser {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn parse<'a>(&self, input: &'a [u8]) -> Result<(&'a [u8], Message), ParseError> {
        let (input, _start) =
            parse::message_start(input).map_err(|err| ParseError::expect(err, "message_start"))?;

        let (input, _frame_length) =
            parse::frame_length(input).map_err(|err| ParseError::expect(err, "frame_length"))?;

        let (input, message_type) =
            parse::message_type(input).map_err(|err| ParseError::expect(err, "message_type"))?;
        let message_type = MessageType::try_from(message_type)?;

        match message_type {
            MessageType::Ping => {
                let parse_time = |input| -> Result<(&[u8], Option<DateTime<Utc>>), ParseError> {
                    let (input, t) =
                        parse::time(input).map_err(|err| ParseError::expect(err, "time"))?;
                    let rfc3339 = String::from_utf8(t.to_vec())?;
                    let t = DateTime::parse_from_rfc3339(&rfc3339).unwrap();
                    Ok((input, Some(t.with_timezone(&Utc))))
                };

                let (input, pre) =
                    parse::prefix(input).map_err(|err| ParseError::expect(err, "prefix"))?;

                println!("prefix {pre}");

                let (input, client) = match pre {
                    prefix::TIME => parse_time(input)?,
                    prefix::NULL => (input, None),
                    _ => unreachable!(),
                };
                let (input, server) = match pre {
                    prefix::TIME => parse_time(input)?,
                    prefix::NULL => (input, None),
                    _ => unreachable!(),
                };
                Ok((
                    input,
                    Message::Ping(Ping {
                        client_timestamp: client,
                        server_timestamp: server,
                    }),
                ))
            }
            MessageType::Authenticate => {
                let (input, (username, password)) = parse::authenticate(input)
                    .map_err(|err| ParseError::expect(err, "message authenticate"))?;
                let username = String::from_utf8(username.to_vec())?;
                let password = String::from_utf8(password.to_vec())?;
                Ok((
                    input,
                    Message::Authenticate(Authenticate::new(username, password)),
                ))
            }
            MessageType::Success => todo!(),
            MessageType::Fail => todo!(),
            MessageType::Set => todo!(),
            MessageType::Get => todo!(),
            MessageType::Delete => todo!(),
        }
    }
}

mod parse {
    use nom::{
        bytes::streaming::{tag, take},
        combinator::map,
        number::streaming::{be_u64, u8},
        sequence::{pair, preceded, terminated},
        IResult, Parser as _,
    };

    use crate::message::{frame::prefix, spec};
    pub(super) fn message_start(input: &[u8]) -> IResult<&[u8], &[u8]> {
        tag([prefix::MESSAGE_START].as_slice())(input)
    }

    pub(super) fn frame_length(input: &[u8]) -> IResult<&[u8], u64> {
        preceded(tag([prefix::FRAME_LENGTH].as_slice()), u64).parse(input)
    }

    pub(super) fn message_type(input: &[u8]) -> IResult<&[u8], u8> {
        preceded(tag([prefix::MESSAGE_TYPE].as_slice()), u8).parse(input)
    }

    pub(super) fn authenticate(input: &[u8]) -> IResult<&[u8], (&[u8], &[u8])> {
        pair(string, string).parse(input)
    }

    pub(super) fn prefix(input: &[u8]) -> IResult<&[u8], u8> {
        u8(input)
    }

    fn delimiter(input: &[u8]) -> IResult<&[u8], ()> {
        map(tag(spec::DELIMITER), |_| ()).parse(input)
    }

    fn u64(input: &[u8]) -> IResult<&[u8], u64> {
        be_u64(input)
    }

    #[allow(dead_code)]
    fn string(input: &[u8]) -> IResult<&[u8], &[u8]> {
        let (input, len) = preceded(tag([prefix::STRING].as_slice()), u64).parse(input)?;
        terminated(take(len), delimiter).parse(input)
    }

    pub(super) fn time(input: &[u8]) -> IResult<&[u8], &[u8]> {
        let (input, len) = preceded(tag([prefix::TIME].as_slice()), u64).parse(input)?;
        terminated(take(len), delimiter).parse(input)
    }

    #[cfg(test)]
    mod tests {
        use chrono::DateTime;

        use crate::message::{frame::Frame, MessageType};

        use super::*;

        #[tokio::test]
        async fn parse_message_start() {
            let mut buf = Vec::new();
            let f = Frame::MessageStart;
            f.write(&mut buf).await.unwrap();

            let (remain, message) = message_start(buf.as_slice()).unwrap();
            assert!(remain.is_empty());
            assert_eq!(message, [prefix::MESSAGE_START].as_slice());

            let err = message_start(b"").unwrap_err();
            assert!(err.is_incomplete());
        }

        #[tokio::test]
        async fn parse_frame_length() {
            let mut buf = Vec::new();
            let f = Frame::Length(100);
            f.write(&mut buf).await.unwrap();

            let (remain, length) = frame_length(buf.as_slice()).unwrap();
            assert_eq!(length, 100);
            assert!(remain.is_empty());

            let err = frame_length(b"").unwrap_err();
            assert!(err.is_incomplete());
        }

        #[tokio::test]
        async fn parse_message_type() {
            let mut buf = Vec::new();
            let auth = MessageType::Authenticate;
            let f = Frame::MessageType(auth); // Replace `SomeType` with an actual variant of `MessageType`
            f.write(&mut buf).await.unwrap();

            let (remain, mt) = message_type(buf.as_slice()).unwrap();
            assert_eq!(mt, auth.into()); // Ensure `SomeType` matches the variant used above
            assert!(remain.is_empty());

            let err = message_type(b"").unwrap_err();
            assert!(err.is_incomplete());
        }

        #[tokio::test]
        async fn parse_string_frame() {
            for string_data in ["Hello", "", "\r\n"] {
                let mut buf = Vec::new();
                let f = Frame::String(string_data.to_owned());
                f.write(&mut buf).await.unwrap();

                let (remain, parsed_string) = string(buf.as_slice()).unwrap();
                assert_eq!(parsed_string, string_data.as_bytes());
                assert!(remain.is_empty());
            }
            let err = string(b"").unwrap_err();
            assert!(err.is_incomplete());
        }

        #[tokio::test]
        async fn parse_time_frame() {
            let mut buf = Vec::new();
            let t = DateTime::from_timestamp(1000, 0).unwrap();
            let f = Frame::Time(t);
            f.write(&mut buf).await.unwrap();

            let (remain, parsed_time) = time(buf.as_slice()).unwrap();
            assert_eq!(parsed_time, t.to_rfc3339().as_bytes());
            assert!(remain.is_empty());

            let err = time(b"").unwrap_err();
            assert!(err.is_incomplete());
        }
    }
}
