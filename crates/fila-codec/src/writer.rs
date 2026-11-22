//! Writing fields into a caller-supplied buffer.
//!
//! Mirrors [`crate::Reader`]: every method advances the cursor itself, so a field
//! cannot be written without the cursor moving past it. `None` means the buffer ran out
//! of room, which is the only way writing can fail — a [`String8`] cannot exceed what
//! its length prefix describes, and no other field here carries a length.
//!
//! On `None` the bytes written before the failure are left in the buffer. Nothing can be
//! assumed about its contents unless a write returns `Some`.

use crate::{LENGTH_PREFIX_LEN, String8};

pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// How many bytes have been written.
    pub fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> Option<()> {
        self.buf
            .get_mut(self.pos..self.pos + bytes.len())?
            .copy_from_slice(bytes);
        self.pos += bytes.len();
        Some(())
    }

    pub(crate) fn u8(&mut self, value: impl Into<u8>) -> Option<()> {
        self.bytes(&value.into().to_be_bytes())
    }

    pub(crate) fn u16(&mut self, value: impl Into<u16>) -> Option<()> {
        self.bytes(&value.into().to_be_bytes())
    }

    pub(crate) fn u32(&mut self, value: impl Into<u32>) -> Option<()> {
        self.bytes(&value.into().to_be_bytes())
    }

    pub(crate) fn u64(&mut self, value: impl Into<u64>) -> Option<()> {
        self.bytes(&value.into().to_be_bytes())
    }

    /// Writes `f`'s output behind a `u32` length that counts it.
    ///
    /// The length is not known until `f` has run, so four bytes are claimed first and
    /// filled in afterwards. Pairing the two inside one method is what keeps a frame from
    /// being written behind a length nobody set, and leaves the only arithmetic over the
    /// cursor in one place.
    pub(crate) fn length_prefixed<F>(&mut self, f: F) -> Option<()>
    where
        F: FnOnce(&mut Self) -> Option<()>,
    {
        let slot = self.pos;
        self.u32(0u32)?;

        f(self)?;

        let len = u32::try_from(self.pos - slot - LENGTH_PREFIX_LEN).ok()?;
        self.buf
            .get_mut(slot..slot + LENGTH_PREFIX_LEN)?
            .copy_from_slice(&len.to_be_bytes());
        Some(())
    }

    /// Writes the `[u8 present]` byte that opens an `optional<T>`.
    pub(crate) fn presence_flag(&mut self, present: bool) -> Option<()> {
        self.u8(u8::from(present))
    }

    /// Writes a `string8`: `[u8 length][UTF-8 bytes]`.
    ///
    /// The only cast of a length to its prefix width, and `String8`'s constructors are
    /// what make it lossless.
    pub(crate) fn string8(&mut self, value: &String8<'_>) -> Option<()> {
        self.u8(value.wire_len())?;
        self.bytes(value.as_bytes())
    }

    pub(crate) fn optional_string8(&mut self, value: &Option<String8<'_>>) -> Option<()> {
        match value {
            Some(s) => {
                self.presence_flag(true)?;
                self.string8(s)
            }
            None => self.presence_flag(false),
        }
    }
}
