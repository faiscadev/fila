//! The two frames a connection opens with.
//!
//! Each type reads and writes itself, so the field order on both sides sits in one
//! place. Encode and decode drifting apart on field order is the one fault a round trip
//! cannot catch — swap two same-typed fields in both and it still passes.

use thiserror::Error;

use crate::{Reader, String8, String8DecodeError, Writer};

#[derive(Debug, PartialEq)]
pub struct Handshake<'a> {
    pub min_version: u16,
    pub max_version: u16,
    pub api_key: Option<String8<'a>>,
    pub client_capabilities: u32,
}

#[derive(Debug, PartialEq, Error)]
pub enum HandshakeError {
    #[error("no min_version")]
    MissingMinVersion,
    #[error("no max_version")]
    MissingMaxVersion,
    #[error("body ended inside the api key")]
    MissingApiKey,
    #[error("api key presence flag is {0}, which is neither 0 nor 1")]
    InvalidApiKeyPresenceFlag(u8),
    #[error("api key: {0}")]
    InvalidApiKey(#[from] String8DecodeError),
    #[error("no client_capabilities")]
    MissingClientCapabilities,
}

impl<'a> Handshake<'a> {
    pub(crate) fn read(reader: &mut Reader<'a>) -> Result<Self, HandshakeError> {
        let min_version = reader.u16().ok_or(HandshakeError::MissingMinVersion)?;
        let max_version = reader.u16().ok_or(HandshakeError::MissingMaxVersion)?;
        let api_key = if reader
            .presence_flag()
            .ok_or(HandshakeError::MissingApiKey)?
            .map_err(HandshakeError::InvalidApiKeyPresenceFlag)?
        {
            Some(
                reader
                    .string8()
                    .ok_or(HandshakeError::MissingApiKey)?
                    .map_err(HandshakeError::InvalidApiKey)?,
            )
        } else {
            None
        };
        let client_capabilities = reader
            .u32()
            .ok_or(HandshakeError::MissingClientCapabilities)?;

        Ok(Self {
            min_version,
            max_version,
            api_key,
            client_capabilities,
        })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        writer.u16(self.min_version)?;
        writer.u16(self.max_version)?;
        writer.optional_string8(&self.api_key)?;
        writer.u32(self.client_capabilities)
    }
}

#[derive(Debug, PartialEq)]
pub struct HandshakeOk {
    pub negotiated_version: u16,
    pub node_id: u64,        // 0 if single-node
    pub max_frame_size: u32, // 0 = default 16 MiB
    pub server_capabilities: u32,
}

#[derive(Debug, PartialEq, Error)]
pub enum HandshakeOkError {
    #[error("no negotiated_version")]
    MissingNegotiatedVersion,
    #[error("no node_id")]
    MissingNodeId,
    #[error("no max_frame_size")]
    MissingMaxFrameSize,
    #[error("no server_capabilities")]
    MissingServerCapabilities,
}

impl HandshakeOk {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, HandshakeOkError> {
        let negotiated_version = reader
            .u16()
            .ok_or(HandshakeOkError::MissingNegotiatedVersion)?;
        let node_id = reader.u64().ok_or(HandshakeOkError::MissingNodeId)?;
        let max_frame_size = reader.u32().ok_or(HandshakeOkError::MissingMaxFrameSize)?;
        let server_capabilities = reader
            .u32()
            .ok_or(HandshakeOkError::MissingServerCapabilities)?;

        Ok(Self {
            negotiated_version,
            node_id,
            max_frame_size,
            server_capabilities,
        })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        writer.u16(self.negotiated_version)?;
        writer.u64(self.node_id)?;
        writer.u32(self.max_frame_size)?;
        writer.u32(self.server_capabilities)
    }
}
