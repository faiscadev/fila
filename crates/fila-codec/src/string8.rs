use core::borrow::Borrow;
use core::fmt;
use core::ops::Deref;
use core::str::Utf8Error;

use alloc::borrow::Cow;
use alloc::string::String;
use thiserror::Error;

/// A UTF-8 string that fits a `string8` on the wire: at most 255 bytes, borrowed from
/// the buffer it was decoded from or owned by whoever built it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct String8<'a>(Cow<'a, str>);

/// A string too long for a one-byte length prefix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{len} bytes exceeds the {} a string8 can describe", String8::MAX_LEN)]
pub struct TooLongError {
    len: usize,
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
}

impl String8<'_> {
    pub const MAX_LEN: usize = u8::MAX as usize;

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Cannot narrow: construction bounded the length.
    pub fn wire_len(&self) -> u8 {
        self.0.len() as u8
    }
}

impl String8<'_> {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Takes ownership of the bytes, copying them only if they are still borrowed.
    pub fn into_owned(self) -> String {
        self.0.into_owned()
    }
}

impl<'a> String8<'a> {
    /// Builds from wire bytes whose length a `u8` prefix has already bounded, so the
    /// only way left to fail is UTF-8.
    pub(crate) fn from_wire(bytes: &'a [u8]) -> Result<Self, Utf8Error> {
        debug_assert!(
            bytes.len() <= Self::MAX_LEN,
            "a u8 length prefix cannot describe {} bytes",
            bytes.len()
        );
        Ok(Self(Cow::Borrowed(core::str::from_utf8(bytes)?)))
    }
}

impl<'a> TryFrom<&'a str> for String8<'a> {
    type Error = TooLongError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX_LEN {
            Err(TooLongError { len: value.len() })
        } else {
            Ok(Self(Cow::Borrowed(value)))
        }
    }
}

impl TryFrom<String> for String8<'_> {
    type Error = TooLongError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > Self::MAX_LEN {
            Err(TooLongError { len: value.len() })
        } else {
            Ok(Self(Cow::Owned(value)))
        }
    }
}

impl Deref for String8<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for String8<'_> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lets a map keyed by `String8` be looked up with a `&str`, which is sound because
/// `String8`'s `Ord`, `Eq` and `Hash` all delegate to the `str` it wraps.
impl Borrow<str> for String8<'_> {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for String8<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String8<'_>> for String {
    fn from(value: String8<'_>) -> Self {
        value.0.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::string::String;

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
        let held = repeat('a', String8::MAX_LEN);
        let s = String8::try_from(held.as_str()).expect("255 bytes is legal");
        assert_eq!(s.len(), 255);
        assert_eq!(s.wire_len(), 255);
    }

    #[test]
    fn one_byte_too_many_is_refused() {
        let held = repeat('a', String8::MAX_LEN + 1);
        let err = String8::try_from(held.as_str())
            .expect_err("256 bytes has no length prefix that can describe it");
        assert_eq!(err.len(), 256);
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
        let held = repeat('é', 200);
        let err = String8::try_from(held.as_str()).expect_err("400 bytes does not fit");
        assert_eq!(err.len(), 400);

        // and 127 of them is 254 bytes, which does
        let held = repeat('é', 127);
        let s = String8::try_from(held.as_str()).expect("254 bytes is legal");
        assert_eq!(s.wire_len(), 254);
    }

    #[test]
    fn it_can_key_a_map_looked_up_by_str() {
        // `Borrow<str>` is what makes `map.get("name")` work without building a
        // `String8` for every lookup.
        let mut map = alloc::collections::BTreeMap::new();
        map.insert(String8::try_from("traceparent").unwrap(), "00-abc-def-01");

        assert_eq!(map.get("traceparent"), Some(&"00-abc-def-01"));
        assert_eq!(map.get("tenant"), None);
    }

    #[test]
    fn it_reads_as_a_str() {
        let s = String8::try_from("orders").unwrap();
        assert_eq!(s.as_str(), "orders");
        assert_eq!(&*s, "orders", "Deref<Target = str>");
        assert!(s.starts_with("order"), "str methods reach through Deref");
    }

    #[test]
    fn it_borrows_rather_than_copies() {
        let held = repeat('a', 8);
        let s = String8::try_from(held.as_str()).unwrap();
        assert!(core::ptr::eq(s.as_str(), held.as_str()));
    }

    #[test]
    fn it_can_own_its_bytes_instead() {
        // An encoder building a string at runtime has nothing to borrow from, so the
        // owned half is what lets it hold one without a buffer to point into.
        let s = String8::try_from(repeat('a', 8)).expect("8 bytes is legal");
        assert_eq!(s.as_str(), "aaaaaaaa");
        assert_eq!(s.wire_len(), 8);
    }

    #[test]
    fn an_owned_string8_outlives_the_bytes_it_was_built_from() {
        let s = {
            let held = repeat('b', 4);
            String8::try_from(held).expect("4 bytes is legal")
        };
        assert_eq!(s.as_str(), "bbbb");
    }

    #[test]
    fn a_string_too_long_is_refused_however_it_arrives() {
        let err = String8::try_from(repeat('a', 256)).expect_err("256 bytes does not fit");
        assert_eq!(err.len(), 256);
    }

    #[test]
    fn borrowed_and_owned_compare_as_their_contents() {
        let held = alloc::string::String::from("orders");
        let borrowed = String8::try_from(held.as_str()).unwrap();
        let owned = String8::try_from(held.clone()).unwrap();
        assert_eq!(
            borrowed, owned,
            "where the bytes live is not part of the value"
        );
    }
}
