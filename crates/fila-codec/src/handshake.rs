//! The two frames a connection opens with.
//!
//! Each type reads and writes itself, so the field order on both sides sits in one
//! place. Encode and decode drifting apart on field order is the one fault a round trip
//! cannot catch — swap two same-typed fields in both and it still passes.

use core::fmt;

use crate::field::{FieldFault, ended, fault, presence};
use crate::{Reader, String8, Writer};

#[derive(Debug, PartialEq)]
pub struct Handshake<'a> {
    pub min_version: u16,
    pub max_version: u16,
    pub api_key: Option<String8<'a>>,
    pub client_capabilities: u32,
}

pub type HandshakeError = FieldFault<HandshakeField>;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum HandshakeField {
    MinVersion,
    MaxVersion,
    ApiKey,
    ClientCapabilities,
}

impl fmt::Display for HandshakeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MinVersion => "min_version",
            Self::MaxVersion => "max_version",
            Self::ApiKey => "api_key",
            Self::ClientCapabilities => "client_capabilities",
        })
    }
}

impl<'a> Handshake<'a> {
    pub(crate) fn read(reader: &mut Reader<'a>) -> Result<Self, HandshakeError> {
        use HandshakeField as F;

        let min_version = reader.u16().ok_or(ended(F::MinVersion))?;
        let max_version = reader.u16().ok_or(ended(F::MaxVersion))?;
        let api_key = if reader
            .presence_flag()
            .ok_or(ended(F::ApiKey))?
            .map_err(presence(F::ApiKey))?
        {
            Some(
                reader
                    .string8()
                    .ok_or(ended(F::ApiKey))?
                    .map_err(fault(F::ApiKey))?,
            )
        } else {
            None
        };
        let client_capabilities = reader.u32().ok_or(ended(F::ClientCapabilities))?;

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

pub type HandshakeOkError = FieldFault<HandshakeOkField>;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum HandshakeOkField {
    NegotiatedVersion,
    NodeId,
    MaxFrameSize,
    ServerCapabilities,
}

impl fmt::Display for HandshakeOkField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NegotiatedVersion => "negotiated_version",
            Self::NodeId => "node_id",
            Self::MaxFrameSize => "max_frame_size",
            Self::ServerCapabilities => "server_capabilities",
        })
    }
}

impl HandshakeOk {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, HandshakeOkError> {
        use HandshakeOkField as F;

        let negotiated_version = reader.u16().ok_or(ended(F::NegotiatedVersion))?;
        let node_id = reader.u64().ok_or(ended(F::NodeId))?;
        let max_frame_size = reader.u32().ok_or(ended(F::MaxFrameSize))?;
        let server_capabilities = reader.u32().ok_or(ended(F::ServerCapabilities))?;

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
