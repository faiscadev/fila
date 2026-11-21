use core::fmt;
use core::ops::Deref;

use alloc::string::String;
use thiserror::Error;

/// A UTF-8 string that fits a `string8` on the wire: at most 255 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct String8(String);

/// A string too long for a one-byte length prefix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{len} bytes exceeds the {} a string8 can describe", String8::MAX_LEN)]
pub struct TooLongError {
    len: usize,
    value: String,
}

impl TooLongError {
    /// The length that was refused, in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the refused value was empty. Never true since an empty string is legal
    /// but clippy wants this method to exist for symmetry with `len()`.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Takes back the string that was refused.
    pub fn into_value(self) -> String {
        self.value
    }
}

impl String8 {
    pub const MAX_LEN: usize = u8::MAX as usize;

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn wire_len(&self) -> u8 {
        self.0.len() as u8
    }

    /// Builds from wire bytes whose length a `u8` prefix has already bounded, so the
    /// only way left to fail is UTF-8.
    ///
    /// A decoder reading a `string8` cannot produce more than [`Self::MAX_LEN`] bytes,
    /// so [`Self::from_utf8`]'s `TooLongError` arm is unreachable there. Taking this
    /// constructor instead keeps that impossible case out of the decoder's error tree.
    pub(crate) fn from_wire(bytes: &[u8]) -> Result<Self, core::str::Utf8Error> {
        debug_assert!(
            bytes.len() <= Self::MAX_LEN,
            "a u8 length prefix cannot describe {} bytes",
            bytes.len()
        );
        Ok(Self(String::from(core::str::from_utf8(bytes)?)))
    }
}

impl TryFrom<String> for String8 {
    type Error = TooLongError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let len = value.len();
        if len > Self::MAX_LEN {
            Err(TooLongError { len, value })
        } else {
            Ok(Self(value))
        }
    }
}

impl TryFrom<&str> for String8 {
    type Error = TooLongError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(String::from(value))
    }
}

impl Deref for String8 {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for String8 {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for String8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String8> for String {
    fn from(value: String8) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repeat(c: char, n: usize) -> String {
        let mut s = String::new();
        for _ in 0..n {
            s.push(c);
        }
        s
    }

    #[test]
    fn the_longest_describable_string_is_accepted() {
        // MAX_LEN is the largest legal length, not the first illegal one.
        let s = String8::try_from(repeat('a', String8::MAX_LEN)).expect("255 bytes is legal");
        assert_eq!(s.len(), 255);
        assert_eq!(s.wire_len(), 255);
    }

    #[test]
    fn one_byte_too_many_is_refused() {
        let err = String8::try_from(repeat('a', String8::MAX_LEN + 1))
            .expect_err("256 bytes has no length prefix that can describe it");
        assert_eq!(err.len(), 256);
        assert_eq!(
            err.into_value().len(),
            256,
            "the refused value is handed back"
        );
    }

    #[test]
    fn an_empty_string_is_legal() {
        // protocol.md requires an absent `optional` and a present-but-empty `string`
        // to decode differently, so the empty string has to be a value this can hold.
        let s = String8::try_from("").expect("empty is a string8");
        assert!(s.is_empty());
        assert_eq!(s.wire_len(), 0);
    }

    #[test]
    fn the_limit_counts_bytes_not_characters() {
        // 'é' is two bytes in UTF-8, so 200 of them is 400 bytes and does not fit,
        // even though it is well under 255 characters.
        let err = String8::try_from(repeat('é', 200)).expect_err("400 bytes does not fit");
        assert_eq!(err.len(), 400);

        // and 127 of them is 254 bytes, which does
        let s = String8::try_from(repeat('é', 127)).expect("254 bytes is legal");
        assert_eq!(s.wire_len(), 254);
    }

    #[test]
    fn it_reads_as_a_str() {
        let s = String8::try_from("orders").unwrap();
        assert_eq!(s.as_str(), "orders");
        assert_eq!(&*s, "orders", "Deref<Target = str>");
        assert!(s.starts_with("order"), "str methods reach through Deref");
    }
}
