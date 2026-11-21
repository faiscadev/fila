use alloc::vec::Vec;
use thiserror::Error;

use crate::{Reader, String8, Writer};

#[derive(Debug, PartialEq)]
pub struct Enqueue {
    pub queue: String8,
    pub headers: Vec<(String8, String8)>,
    pub fairness_key: Option<String8>,
    pub weight: Option<u32>,
    pub delay_ms: Option<u64>,
    pub idempotency_key: Option<String8>,
}

#[derive(Debug, PartialEq, Error)]
pub enum EnqueueError {}

impl Enqueue {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, EnqueueError> {
        todo!("implement Enqueue::read");
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        todo!("implement Enqueue::write");
    }
}
