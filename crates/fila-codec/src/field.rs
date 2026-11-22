//! One fault, in one field, shared by every frame that reads fields.

use core::fmt;

use thiserror::Error;

/// Which field failed, and how. Both halves are typed, so a caller matches on them
/// rather than on a message.
#[derive(Debug, PartialEq, Error)]
#[error("{field}: {source}")]
pub struct FieldFault<F: fmt::Debug + fmt::Display> {
    pub field: F,
    #[source]
    pub source: FieldError,
}

/// What went wrong in a field, independent of which field it was.
#[derive(Debug, PartialEq, Error)]
pub enum FieldError {
    #[error("the body ended inside it")]
    Ended,
    #[error("presence flag is {0}, which is neither 0 nor 1")]
    PresenceFlag(u8),
    #[error("expected {expected} bytes, found {found}")]
    Truncated { expected: usize, found: usize },
    #[error("{0}")]
    String8(#[from] crate::String8DecodeError),
}

pub(crate) fn ended<F: fmt::Debug + fmt::Display>(field: F) -> FieldFault<F> {
    FieldFault {
        field,
        source: FieldError::Ended,
    }
}

pub(crate) fn presence<F: fmt::Debug + fmt::Display>(field: F) -> impl FnOnce(u8) -> FieldFault<F> {
    move |flag| FieldFault {
        field,
        source: FieldError::PresenceFlag(flag),
    }
}

pub(crate) fn fault<F: fmt::Debug + fmt::Display, E: Into<FieldError>>(
    field: F,
) -> impl FnOnce(E) -> FieldFault<F> {
    move |e| FieldFault {
        field,
        source: e.into(),
    }
}
