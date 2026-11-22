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
    #[error("expected {expected} bytes, found {found}")]
    Truncated { expected: u8, found: usize },
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

    /// Bytes left in the body, which is the `found` half of a truncation error.
    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
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
    pub(crate) fn string8(&mut self) -> Option<Result<String8<'a>, String8DecodeError>> {
        let len = self.u8()? as usize;

        if self.pos + len > self.buf.len() {
            return Some(Err(String8DecodeError::Truncated {
                expected: len as u8,
                found: self.buf.len() - self.pos,
            }));
        }

        let string_bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;

        Some(String8::from_wire(string_bytes).map_err(String8DecodeError::InvalidUtf8))
    }

    /// Reads `len` bytes from the buffer, returning a slice to them.
    pub(crate) fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let next_pos = self.pos.checked_add(len)?;
        if next_pos > self.buf.len() {
            return None;
        }
        let bytes = &self.buf[self.pos..next_pos];
        self.pos = next_pos;
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_of_zero_is_an_empty_slice() {
        // An empty payload is a length of zero, and it can sit at the very end of a
        // body - so a zero-length read at the end is not a failure.
        let mut reader = Reader::new(&[]);
        assert_eq!(reader.bytes(0), Some(&[][..]));
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn bytes_can_take_exactly_what_is_left() {
        let buf = [1, 2, 3];
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.bytes(3), Some(&buf[..]));
        assert_eq!(reader.remaining(), 0);
        assert_eq!(reader.bytes(1), None);
    }

    #[test]
    fn a_read_past_the_end_leaves_the_cursor_alone() {
        // The caller turns `None` into an error naming the field, and reports what was
        // left with `remaining`. Both are wrong if a failed read consumed anything.
        let buf = [1, 2, 3];
        let mut reader = Reader::new(&buf);

        assert_eq!(reader.bytes(4), None);
        assert_eq!(reader.remaining(), 3, "nothing was consumed");
        assert_eq!(reader.bytes(3), Some(&buf[..]));
    }

    #[test]
    fn the_slice_is_the_buffer_and_not_a_copy() {
        let buf = [1, 2, 3, 4];
        let mut reader = Reader::new(&buf);
        let bytes = reader.bytes(4).expect("four bytes are there");
        assert!(core::ptr::eq(bytes, &buf[..]));
    }

    #[test]
    fn a_length_no_buffer_could_satisfy_is_a_failed_read() {
        // `bytes` is the only read whose length comes off the wire: `read_payload`
        // hands it a `u32` straight from the body. `pos + len` overflows before the
        // guard can refuse it, which on a 32-bit target a declared payload of
        // `u32::MAX` reaches.
        let buf = [1, 2, 3];
        let mut reader = Reader::new(&buf);
        reader.u8().expect("one byte is there");

        assert_eq!(reader.bytes(usize::MAX), None);
        assert_eq!(reader.remaining(), 2, "a failed read consumes nothing");
    }

    #[test]
    fn remaining_counts_down_as_fields_are_read() {
        let buf = [0, 0, 0, 1, 0xFF];
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.remaining(), 5);
        reader.u32().expect("four bytes are there");
        assert_eq!(reader.remaining(), 1);
        reader.u8().expect("one byte is there");
        assert_eq!(reader.remaining(), 0);
    }
}
