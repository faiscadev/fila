use core::fmt;
use core::ops::Deref;

use alloc::vec::Vec;
use thiserror::Error;

use crate::field::{FieldError, FieldFault, ended, fault, presence};
use crate::{Headers, Payload, Reader, String8, Writer};

/// The messages one `Enqueue` frame carries.
///
/// A `Vec` rather than a `&[EnqueueMessage]`: the buffer a frame is decoded from holds
/// wire bytes, not parsed messages, so a decoder has nowhere to put an array it could
/// then lend out. Each message's payload still borrows from that buffer, so this is one
/// allocation per frame and none per byte. Lending the messages too would mean either
/// scratch space from the caller or an iterator that parses on demand.
#[derive(Debug, PartialEq)]
pub struct Enqueue<'a>(pub Vec<EnqueueMessage<'a>>);

impl<'a> From<Vec<EnqueueMessage<'a>>> for Enqueue<'a> {
    fn from(messages: Vec<EnqueueMessage<'a>>) -> Self {
        Self(messages)
    }
}

impl<'a> FromIterator<EnqueueMessage<'a>> for Enqueue<'a> {
    fn from_iter<I: IntoIterator<Item = EnqueueMessage<'a>>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> Deref for Enqueue<'a> {
    type Target = [EnqueueMessage<'a>];

    fn deref(&self) -> &[EnqueueMessage<'a>] {
        &self.0
    }
}

impl<'a> IntoIterator for Enqueue<'a> {
    type Item = EnqueueMessage<'a>;
    type IntoIter = alloc::vec::IntoIter<EnqueueMessage<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Enqueue<'a> {
    type Item = &'a EnqueueMessage<'a>;
    type IntoIter = core::slice::Iter<'a, EnqueueMessage<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug, PartialEq)]
pub struct EnqueueMessage<'a> {
    pub queue: String8<'a>,
    pub headers: Headers<'a>,
    pub payload: Payload<'a>,
    pub fairness_key: Option<String8<'a>>,
    pub weight: Option<u32>,
    pub delay_ms: Option<u64>,
    pub idempotency_key: Option<String8<'a>>,
}

#[derive(Debug, PartialEq, Error)]
pub enum InvalidHeaderError {
    #[error("missing key")]
    MissingKey,
    #[error("key: {0}")]
    InvalidKey(#[source] crate::String8DecodeError),
    #[error("missing value")]
    MissingValue,
    #[error("value: {0}")]
    InvalidValue(#[source] crate::String8DecodeError),
    #[error("{0}")]
    DuplicateKey(#[from] crate::DuplicateKeyError),
}

#[derive(Debug, PartialEq, Error)]
pub enum EnqueueError {
    #[error("no message_count")]
    MissingMessageCount,

    #[error("message {index}: {fault}")]
    Message {
        index: usize,
        #[source]
        fault: FieldFault<MessageField>,
    },
}

/// The fields of one message, named as protocol.md names them.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum MessageField {
    Queue,
    Headers,
    Payload,
    FairnessKey,
    Weight,
    DelayMs,
    IdempotencyKey,
}

impl fmt::Display for MessageField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Queue => "queue",
            Self::Headers => "headers",
            Self::Payload => "payload",
            Self::FairnessKey => "fairness_key",
            Self::Weight => "weight",
            Self::DelayMs => "delay_ms",
            Self::IdempotencyKey => "idempotency_key",
        })
    }
}

impl<'a> EnqueueMessage<'a> {
    pub(crate) fn read(reader: &mut Reader<'a>) -> Result<Self, FieldFault<MessageField>> {
        use MessageField as F;

        let queue = reader
            .string8()
            .ok_or(ended(F::Queue))?
            .map_err(fault(F::Queue))?;

        let headers = read_headers(reader)
            .ok_or(ended(F::Headers))?
            .map_err(fault(F::Headers))?;

        let payload =
            read_payload(reader)
                .ok_or(ended(F::Payload))?
                .map_err(|(expected, found)| FieldFault {
                    field: F::Payload,
                    source: FieldError::Truncated { expected, found },
                })?;

        let fairness_key = if reader
            .presence_flag()
            .ok_or(ended(F::FairnessKey))?
            .map_err(presence(F::FairnessKey))?
        {
            Some(
                reader
                    .string8()
                    .ok_or(ended(F::FairnessKey))?
                    .map_err(fault(F::FairnessKey))?,
            )
        } else {
            None
        };

        let weight = if reader
            .presence_flag()
            .ok_or(ended(F::Weight))?
            .map_err(presence(F::Weight))?
        {
            Some(reader.u32().ok_or(ended(F::Weight))?)
        } else {
            None
        };

        let delay_ms = if reader
            .presence_flag()
            .ok_or(ended(F::DelayMs))?
            .map_err(presence(F::DelayMs))?
        {
            Some(reader.u64().ok_or(ended(F::DelayMs))?)
        } else {
            None
        };

        let idempotency_key = if reader
            .presence_flag()
            .ok_or(ended(F::IdempotencyKey))?
            .map_err(presence(F::IdempotencyKey))?
        {
            Some(
                reader
                    .string8()
                    .ok_or(ended(F::IdempotencyKey))?
                    .map_err(fault(F::IdempotencyKey))?,
            )
        } else {
            None
        };

        Ok(Self {
            queue,
            headers,
            payload,
            fairness_key,
            weight,
            delay_ms,
            idempotency_key,
        })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        writer.string8(&self.queue)?;

        write_headers(writer, &self.headers)?;
        write_payload(writer, &self.payload)?;

        match &self.fairness_key {
            Some(fairness_key) => {
                writer.presence_flag(true)?;
                writer.string8(fairness_key)?;
            }
            None => {
                writer.presence_flag(false)?;
            }
        }

        match self.weight {
            Some(weight) => {
                writer.presence_flag(true)?;
                writer.u32(weight)?;
            }
            None => {
                writer.presence_flag(false)?;
            }
        }

        match self.delay_ms {
            Some(delay_ms) => {
                writer.presence_flag(true)?;
                writer.u64(delay_ms)?;
            }
            None => {
                writer.presence_flag(false)?;
            }
        }

        match &self.idempotency_key {
            Some(idempotency_key) => {
                writer.presence_flag(true)?;
                writer.string8(idempotency_key)?;
            }
            None => {
                writer.presence_flag(false)?;
            }
        }

        Some(())
    }
}

impl<'a> Enqueue<'a> {
    pub(crate) fn read(reader: &mut Reader<'a>) -> Result<Self, EnqueueError> {
        let message_count = reader.u32().ok_or(EnqueueError::MissingMessageCount)?;
        let mut messages = Vec::new();
        for index in 0..message_count as usize {
            let message = EnqueueMessage::read(reader)
                .map_err(|fault| EnqueueError::Message { index, fault })?;
            messages.push(message);
        }

        Ok(Self(messages))
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        writer.u32(self.len() as u32)?;
        for message in self {
            message.write(writer)?;
        }

        Some(())
    }
}

fn write_headers(writer: &mut Writer<'_>, headers: &Headers<'_>) -> Option<()> {
    writer.u16(headers.wire_count())?;

    for (key, value) in headers.iter() {
        writer.string8(key)?;
        writer.string8(value)?;
    }

    Some(())
}

fn read_header<'a>(
    reader: &mut Reader<'a>,
) -> Result<(String8<'a>, String8<'a>), InvalidHeaderError> {
    let key = reader
        .string8()
        .ok_or(InvalidHeaderError::MissingKey)?
        .map_err(InvalidHeaderError::InvalidKey)?;

    let value = reader
        .string8()
        .ok_or(InvalidHeaderError::MissingValue)?
        .map_err(InvalidHeaderError::InvalidValue)?;

    Ok((key, value))
}

fn read_headers<'a>(reader: &mut Reader<'a>) -> Option<Result<Headers<'a>, InvalidHeaderError>> {
    let headers_len = reader.u16()? as usize;
    let mut headers = Headers::default();

    for _ in 0..headers_len {
        let (key, value) = match read_header(reader) {
            Ok(header) => header,
            Err(e) => return Some(Err(e)),
        };

        // The count is a `u16`, so this cannot overrun what a `Headers` holds: a
        // repeated key is the only thing left to refuse.
        if let Err(e) = headers.insert_from_wire(key, value) {
            return Some(Err(InvalidHeaderError::DuplicateKey(e)));
        }
    }

    Some(Ok(headers))
}

fn write_payload(writer: &mut Writer<'_>, payload: &Payload) -> Option<()> {
    writer.u32(payload.wire_len())?;
    writer.bytes(payload)?;

    Some(())
}

fn read_payload<'a>(reader: &mut Reader<'a>) -> Option<Result<Payload<'a>, (usize, usize)>> {
    let expected = reader.u32()? as usize;

    match reader.bytes(expected) {
        Some(bytes) => Some(Ok(Payload::from_bounded_bytes(bytes))),
        None => Some(Err((expected, reader.remaining()))),
    }
}
