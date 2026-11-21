//! The frame every operation travels in: a header, and a body chosen by its opcode.

use thiserror::Error;

use crate::{
    FRAME_HEADER_LEN, Handshake, HandshakeError, HandshakeOk, HandshakeOkError, MAX_FRAME_BODY_LEN,
    Reader, Writer,
};

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum Opcode {
    Handshake = 0x01,
    HandshakeOk = 0x02,
    Ping = 0x03,
}

impl From<Opcode> for u8 {
    fn from(opcode: Opcode) -> Self {
        opcode as u8
    }
}

impl TryFrom<u8> for Opcode {
    type Error = OpcodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Handshake),
            0x02 => Ok(Self::HandshakeOk),
            0x03 => Ok(Self::Ping),
            v => Err(OpcodeError::Invalid(v)),
        }
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum OpcodeError {
    #[error("no opcode byte")]
    Missing,
    #[error("unknown opcode {0:#04x}")]
    Invalid(u8),
}

const CONTINUATION: u8 = 0b0000_0001;
const RESERVED: u8 = 0b1111_1110;

/// A flag bit changes how its own frame is read, so an unrecognised one is refused
/// rather than ignored. That is the opposite of a capability bit, which is a standing
/// claim about a peer and costs nothing to disregard.
fn validate_flags(flags: u8) -> Option<FlagsError> {
    if flags & CONTINUATION != 0 {
        Some(FlagsError::ContinuationUnsupported)
    } else if flags & RESERVED != 0 {
        Some(FlagsError::ReservedSet { flags })
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum FlagsError {
    #[error("no flags byte")]
    Missing,
    #[error("continuation frames are not supported")]
    ContinuationUnsupported,
    #[error("reserved flag bits set in {flags:#010b}")]
    ReservedSet { flags: u8 },
}

#[derive(Debug, PartialEq, Error)]
pub enum HeaderError {
    #[error("flags: {0}")]
    Flags(#[from] FlagsError),
    #[error("opcode: {0}")]
    Opcode(#[from] OpcodeError),
    #[error("no request id")]
    MissingRequestId,
}

#[derive(Debug, PartialEq)]
pub struct FrameHeader {
    pub opcode: Opcode,
    pub request_id: u32,
}

impl FrameHeader {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, HeaderError> {
        let opcode = reader.u8().ok_or(OpcodeError::Missing)?;
        let opcode: Opcode = opcode.try_into()?;

        // we don't support any flags yet, so just validate none are set
        let flags = reader.u8().ok_or(FlagsError::Missing)?;
        if let Some(err) = validate_flags(flags) {
            return Err(err.into());
        }

        let request_id = reader.u32().ok_or(HeaderError::MissingRequestId)?;

        Ok(Self { opcode, request_id })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        writer.u8(self.opcode)?;
        writer.u8(0b0000_0000u8)?; // flags: continuation is not implemented
        writer.u32(self.request_id)
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum FrameError {
    #[error("frame header: {0}")]
    Header(#[from] HeaderError),
    #[error(
        "declared length {length} is below the {}-byte frame header",
        FRAME_HEADER_LEN
    )]
    LengthBelowHeader { length: u32 },
    #[error(
        "declared length {length} is above the maximum of {}",
        MAX_FRAME_BODY_LEN
    )]
    LengthAboveMaximum { length: u32 },
    #[error("frame body: {0}")]
    Body(#[from] BodyError),
}

#[derive(Debug, PartialEq, Error)]
pub enum BodyError {
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("handshake-ok: {0}")]
    HandshakeOk(#[from] HandshakeOkError),
}

#[derive(Debug, PartialEq)]
pub enum FrameBody {
    Handshake(Handshake),
    HandshakeOk(HandshakeOk),
    Ping,
}

impl FrameBody {
    fn read(reader: &mut Reader<'_>, opcode: Opcode) -> Result<Self, BodyError> {
        Ok(match opcode {
            Opcode::Handshake => Self::Handshake(Handshake::read(reader)?),
            Opcode::HandshakeOk => Self::HandshakeOk(HandshakeOk::read(reader)?),
            Opcode::Ping => Self::Ping,
        })
    }

    fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        match self {
            Self::Handshake(handshake) => handshake.write(writer),
            Self::HandshakeOk(handshake_ok) => handshake_ok.write(writer),
            Self::Ping => Some(()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub request_id: u32,
    pub body: FrameBody,
}

impl Frame {
    /// The opcode this frame travels under, which its body decides. Nothing stores it:
    /// a frame whose opcode disagreed with its body would be unencodable.
    pub fn opcode(&self) -> Opcode {
        match &self.body {
            FrameBody::Handshake(_) => Opcode::Handshake,
            FrameBody::HandshakeOk(_) => Opcode::HandshakeOk,
            FrameBody::Ping => Opcode::Ping,
        }
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, FrameError> {
        let header = FrameHeader::read(reader)?;
        let body = FrameBody::read(reader, header.opcode)?;

        Ok(Self {
            request_id: header.request_id,
            body,
        })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Option<()> {
        let header = FrameHeader {
            opcode: self.opcode(),
            request_id: self.request_id,
        };

        writer.length_prefixed(|w| {
            header.write(w)?;
            self.body.write(w)
        })
    }
}
