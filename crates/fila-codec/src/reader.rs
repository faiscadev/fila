//! Reading fields out of a frame body.
//!
//! Every method is bounds checked against the buffer it was given and advances the
//! cursor itself. `None` means the body ended before the field did, which is this
//! reader's one convention for running out of bytes: a caller turns it into an error
//! named for the field it was reading.

use core::str::Utf8Error;

use thiserror::Error;

use crate::String8;

#[derive(Debug, PartialEq, Error)]
pub enum String8DecodeError {
    #[error("declares {declared} bytes with {available} left in the body")]
    Truncated { declared: u8, available: usize },
    #[error("not utf-8: {0}")]
    InvalidUtf8(#[from] Utf8Error),
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        if self.pos + 1 > self.buf.len() {
            return None;
        }
        let value = self.buf[self.pos];
        self.pos += 1;
        Some(value)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        if self.pos + 2 > self.buf.len() {
            return None;
        }
        let value = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Some(value)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        if self.pos + 4 > self.buf.len() {
            return None;
        }
        let value = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Some(value)
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        if self.pos + 8 > self.buf.len() {
            return None;
        }
        let value = u64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Some(value)
    }

    /// Reads the `[u8 present]` byte that opens an `optional<T>`.
    ///
    /// `Err` carries the byte itself. The protocol defines 0 and 1 and nothing else, and
    /// this byte decides how the bytes behind it are read, so an undefined value cannot
    /// be guessed at. The fault belongs to the `optional` wrapper rather than to `T`,
    /// which is why the byte is read on its own.
    pub(crate) fn presence_flag(&mut self) -> Option<Result<bool, u8>> {
        let byte = self.u8()?;
        Some(match byte {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(other),
        })
    }

    /// Reads a `string8`: `[u8 length][UTF-8 bytes]`.
    pub(crate) fn string8(&mut self) -> Option<Result<String8, String8DecodeError>> {
        let len = self.u8()? as usize;

        if self.pos + len > self.buf.len() {
            return Some(Err(String8DecodeError::Truncated {
                declared: len as u8,
                available: self.buf.len() - self.pos,
            }));
        }

        let string_bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;

        Some(String8::from_wire(string_bytes).map_err(String8DecodeError::InvalidUtf8))
    }
}
