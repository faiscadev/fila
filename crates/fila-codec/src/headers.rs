//! The header map on a message, bounded by the count that describes it.

use core::ops::Deref;

use alloc::collections::BTreeMap;
use alloc::collections::btree_map::Entry;
use alloc::string::String;
use alloc::vec::Vec;
use thiserror::Error;

use crate::String8;

/// The headers of one message: at most 65,535 entries, each key once.
///
/// Held in ascending key order, whatever order they arrived in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers<'a>(BTreeMap<String8<'a>, String8<'a>>);

/// More entries than a two-byte count can describe.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{len} entries exceeds the {} a header map can describe",
    Headers::MAX_LEN
)]
pub struct TooManyError {
    len: usize,
}

impl TooManyError {
    /// The number of entries that was refused.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the refused map was empty. Never true since an empty map is legal, but
    /// clippy wants this method to exist for symmetry with `len()`.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("duplicate key {key:?} with value {value:?}")]
pub struct DuplicateKeyError {
    key: String,
    value: String,
}

impl<'a> Headers<'a> {
    pub const MAX_LEN: usize = u16::MAX as usize;

    /// Cannot narrow: construction bounded the length.
    pub fn wire_count(&self) -> u16 {
        self.0.len() as u16
    }

    /// Adds an entry read off the wire, refusing a key that is already there.
    ///
    /// A `u16` count cannot describe more than [`Self::MAX_LEN`] distinct keys, so a
    /// duplicate is the only way this fails.
    pub(crate) fn insert_from_wire(
        &mut self,
        key: String8<'a>,
        value: String8<'a>,
    ) -> Result<(), DuplicateKeyError> {
        match self.0.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(value);
                Ok(())
            }
            Entry::Occupied(slot) => Err(DuplicateKeyError {
                key: String::from(slot.key().as_str()),
                value: String::from(value.as_str()),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HeadersError {
    #[error("{0}")]
    TooMany(#[from] TooManyError),
    #[error("{0}")]
    DuplicateKey(#[from] DuplicateKeyError),
}

impl<'a> TryFrom<Vec<(String8<'a>, String8<'a>)>> for Headers<'a> {
    type Error = HeadersError;

    fn try_from(value: Vec<(String8<'a>, String8<'a>)>) -> Result<Self, Self::Error> {
        let len = value.len();
        if len > Self::MAX_LEN {
            return Err(TooManyError { len }.into());
        }

        let mut headers = Self::default();
        for (key, value) in value {
            headers.insert_from_wire(key, value)?;
        }

        Ok(headers)
    }
}

impl<'a> Deref for Headers<'a> {
    type Target = BTreeMap<String8<'a>, String8<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> From<Headers<'a>> for BTreeMap<String8<'a>, String8<'a>> {
    fn from(value: Headers<'a>) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` distinct keys, held by the caller: a `String8` borrows, so the strings have
    /// to outlive the map built from them.
    fn keys(n: usize) -> Vec<String> {
        let mut keys = Vec::new();
        for i in 0..n {
            keys.push(alloc::format!("k{i}"));
        }
        keys
    }

    fn entries(keys: &[String]) -> Vec<(String8<'_>, String8<'_>)> {
        let mut entries = Vec::new();
        for key in keys {
            entries.push((
                String8::try_from(key.as_str()).unwrap(),
                String8::try_from("v").unwrap(),
            ));
        }
        entries
    }

    #[test]
    fn the_largest_describable_map_is_accepted() {
        // MAX_LEN is the largest legal count, not the first illegal one.
        let held = keys(Headers::MAX_LEN);
        let headers = Headers::try_from(entries(&held)).expect("65,535 entries is legal");
        assert_eq!(headers.len(), 65_535);
        assert_eq!(headers.wire_count(), 65_535);
    }

    #[test]
    fn one_entry_too_many_is_refused() {
        // 65,536 two-byte entries is 128 KB: the count runs out while the frame is
        // still legal, which is why the limit is here and not in a frame-size check.
        let held = keys(Headers::MAX_LEN + 1);
        let HeadersError::TooMany(err) = Headers::try_from(entries(&held))
            .expect_err("65,536 entries has no count that can describe it")
        else {
            panic!("65,536 distinct keys is a count problem, not a duplicate");
        };
        assert_eq!(err.len(), 65_536);
    }

    #[test]
    fn a_key_read_twice_is_refused_and_named() {
        let mut headers = Headers::default();
        headers
            .insert_from_wire(
                String8::try_from("tenant").unwrap(),
                String8::try_from("acme").unwrap(),
            )
            .expect("the first entry under a key is fine");

        let err = headers
            .insert_from_wire(
                String8::try_from("tenant").unwrap(),
                String8::try_from("other").unwrap(),
            )
            .expect_err("the second is not");

        assert_eq!(
            alloc::format!("{err}"),
            "duplicate key \"tenant\" with value \"other\""
        );
        assert_eq!(
            headers.get("tenant").map(String8::as_str),
            Some("acme"),
            "the entry already there is left alone"
        );
    }

    #[test]
    fn an_empty_map_is_legal() {
        // No presence flag on the map, so "no headers" is a count of zero.
        let headers = Headers::default();
        assert!(headers.is_empty());
        assert_eq!(headers.wire_count(), 0);
    }
}
