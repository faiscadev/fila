//! A message's payload: raw bytes, bounded by the count that describes them.

use core::ops::Deref;

use alloc::borrow::Cow;
use alloc::vec::Vec;
use thiserror::Error;

/// The bytes a message carries, borrowed from the buffer they were decoded from or
/// owned by whoever built them: at most what the `u32` length in front of them can
/// describe.
///
/// That bound is the protocol's, not a frame's - a larger payload travels across
/// continuation frames - so it says nothing about what a connection will accept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Payload<'a>(Cow<'a, [u8]>);

/// More bytes than a four-byte length can describe.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{len} bytes exceeds the {} a payload length can describe",
    Payload::MAX_LEN
)]
pub struct TooLargeError {
    len: usize,
}

impl TooLargeError {
    /// The length that was refused, in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the refused payload was empty. Never true since an empty payload is
    /// legal, but clippy wants this method to exist for symmetry with `len()`.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Payload<'_> {
    pub const MAX_LEN: usize = u32::MAX as usize;

    /// Cannot narrow: construction bounded the length.
    pub fn wire_len(&self) -> u32 {
        self.0.len() as u32
    }
}

impl Payload<'_> {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Takes ownership of the bytes, copying them only if they are still borrowed.
    pub fn into_owned(self) -> Vec<u8> {
        self.0.into_owned()
    }
}

impl<'a> Payload<'a> {
    /// Builds from bytes a `u32` length has already bounded, which keeps
    /// [`TryFrom`]'s unreachable `TooLargeError` arm out of the decoder's error tree.
    pub(crate) fn from_bounded_bytes(bytes: &'a [u8]) -> Self {
        debug_assert!(
            bytes.len() <= Self::MAX_LEN,
            "a u32 length cannot describe {} bytes",
            bytes.len()
        );
        Self(Cow::Borrowed(bytes))
    }
}

impl<'a> TryFrom<&'a [u8]> for Payload<'a> {
    type Error = TooLargeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX_LEN {
            Err(TooLargeError { len: value.len() })
        } else {
            Ok(Self(Cow::Borrowed(value)))
        }
    }
}

impl TryFrom<Vec<u8>> for Payload<'_> {
    type Error = TooLargeError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX_LEN {
            Err(TooLargeError { len: value.len() })
        } else {
            Ok(Self(Cow::Owned(value)))
        }
    }
}

impl Deref for Payload<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Payload<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Payload<'_>> for Vec<u8> {
    fn from(value: Payload<'_>) -> Self {
        value.0.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_is_the_width_of_the_length_prefix() {
        // No test for the first illegal length: it would mean holding 4 GiB.
        assert_eq!(Payload::MAX_LEN, u32::MAX as usize);
    }

    #[test]
    fn an_empty_payload_is_legal() {
        // No presence flag on the payload, so empty is a length of zero.
        let payload = Payload::default();
        assert!(payload.is_empty());
        assert_eq!(payload.wire_len(), 0);
    }

    #[test]
    fn it_carries_bytes_no_string_type_could() {
        // Invalid UTF-8 lead bytes and an interior NUL: `bytes` is not a string.
        static BYTES: [u8; 7] = [0x00, 0xFF, 0x80, 0xC0, 0x00, 0x1B, 0x7F];
        let payload = Payload::try_from(&BYTES[..]).expect("seven bytes is a payload");

        assert_eq!(payload.wire_len(), 7);
        assert_eq!(payload.as_slice(), &BYTES[..]);
        assert_eq!(&*payload, &BYTES[..], "Deref<Target = [u8]>");
    }

    #[test]
    fn it_borrows_rather_than_copies() {
        static BYTES: [u8; 3] = [1, 2, 3];
        let payload = Payload::try_from(&BYTES[..]).unwrap();
        assert!(
            core::ptr::eq(payload.as_slice(), &BYTES[..]),
            "the payload is the same bytes, not a copy of them"
        );
    }

    #[test]
    fn it_can_own_its_bytes_instead() {
        // A producer building a message at runtime has nothing to borrow from.
        let payload = Payload::try_from(alloc::vec![1u8, 2, 3]).expect("3 bytes is a payload");
        assert_eq!(payload.as_slice(), &[1, 2, 3]);
        assert_eq!(payload.wire_len(), 3);
    }

    #[test]
    fn an_owned_payload_outlives_the_bytes_it_was_built_from() {
        let payload = {
            let held = alloc::vec![9u8; 4];
            Payload::try_from(held).expect("4 bytes is a payload")
        };
        assert_eq!(payload.as_slice(), &[9, 9, 9, 9]);
    }

    #[test]
    fn borrowed_and_owned_compare_as_their_contents() {
        static BYTES: [u8; 3] = [1, 2, 3];
        let borrowed = Payload::try_from(&BYTES[..]).unwrap();
        let owned = Payload::try_from(alloc::vec![1u8, 2, 3]).unwrap();
        assert_eq!(
            borrowed, owned,
            "where the bytes live is not part of the value"
        );
    }

    #[test]
    fn it_reads_as_a_slice() {
        let payload = Payload::try_from(&b"hello"[..]).expect("five bytes is a payload");
        assert_eq!(payload.len(), 5, "slice methods reach through Deref");
        assert_eq!(payload.first(), Some(&b'h'));
    }
}
