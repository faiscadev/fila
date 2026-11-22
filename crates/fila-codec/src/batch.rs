//! The items one frame carries: a count, and that many of them.

use core::ops::Deref;

use alloc::vec::Vec;
use thiserror::Error;

/// The items of one batch frame: at most 65,535, which is what the `u16` count in
/// front of them can describe.
///
/// A `Vec` rather than a `&[T]`: the buffer a frame is decoded from holds wire bytes,
/// not parsed items, so a decoder has nowhere to put an array it could then lend out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch<T>(Vec<T>);

/// More items than a two-byte count can describe.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{len} items exceeds the {} a batch count can describe", u16::MAX)]
pub struct TooManyItemsError {
    len: usize,
}

impl TooManyItemsError {
    /// The number of items that was refused.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the refused batch was empty. Never true since an empty batch is legal,
    /// but clippy wants this method to exist for symmetry with `len()`.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T> Batch<T> {
    pub const MAX_LEN: usize = u16::MAX as usize;

    /// Cannot narrow: construction bounded the length.
    pub fn wire_count(&self) -> u16 {
        self.0.len() as u16
    }

    /// Builds from items a `u16` count has already bounded, which keeps [`TryFrom`]'s
    /// unreachable `TooManyItemsError` arm out of the decoder's error tree.
    pub(crate) fn from_bounded(items: Vec<T>) -> Self {
        debug_assert!(
            items.len() <= Self::MAX_LEN,
            "a u16 count cannot describe {} items",
            items.len()
        );
        Self(items)
    }
}

impl<T> TryFrom<Vec<T>> for Batch<T> {
    type Error = TooManyItemsError;

    fn try_from(items: Vec<T>) -> Result<Self, Self::Error> {
        if items.len() > Self::MAX_LEN {
            Err(TooManyItemsError { len: items.len() })
        } else {
            Ok(Self(items))
        }
    }
}

impl<T> Deref for Batch<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> IntoIterator for Batch<T> {
    type Item = T;
    type IntoIter = alloc::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Batch<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T> From<Batch<T>> for Vec<T> {
    fn from(batch: Batch<T>) -> Self {
        batch.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_describable_batch_is_accepted() {
        // MAX_LEN is the largest legal count, not the first illegal one.
        let batch =
            Batch::try_from(alloc::vec![0u8; Batch::<u8>::MAX_LEN]).expect("65,535 items is legal");
        assert_eq!(batch.len(), 65_535);
        assert_eq!(batch.wire_count(), 65_535);
    }

    #[test]
    fn one_item_too_many_is_refused() {
        let err = Batch::try_from(alloc::vec![0u8; Batch::<u8>::MAX_LEN + 1])
            .expect_err("65,536 items has no count that can describe it");
        assert_eq!(err.len(), 65_536);
    }

    #[test]
    fn an_empty_batch_is_legal() {
        let batch = Batch::<u8>::default();
        assert!(batch.is_empty());
        assert_eq!(batch.wire_count(), 0);
    }

    #[test]
    fn it_reads_as_a_slice() {
        let batch = Batch::try_from(alloc::vec![1u8, 2, 3]).unwrap();
        assert_eq!(batch.first(), Some(&1), "slice methods reach through Deref");
        assert_eq!(batch.iter().sum::<u8>(), 6);
    }
}
