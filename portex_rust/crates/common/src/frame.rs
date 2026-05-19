//! Wire framing for the portex control channel.
//!
//! Each control frame is:
//!   [u8 type] [u32 length BE] [payload bytes]
//!
//! Payload format is per-frame-type (see `proto`). Per-request data streams
//! do NOT use this framing — they carry raw HTTP/1.1 bytes end-to-end.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_PAYLOAD: u32 = 64 * 1024;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Hello = 0x01,
    Accept = 0x02,
    Reject = 0x03,
    Ping = 0x04,
    Pong = 0x05,
}

impl FrameType {
    pub fn from_u8(v: u8) -> Result<Self, FrameError> {
        Ok(match v {
            0x01 => FrameType::Hello,
            0x02 => FrameType::Accept,
            0x03 => FrameType::Reject,
            0x04 => FrameType::Ping,
            0x05 => FrameType::Pong,
            other => return Err(FrameError::UnknownType(other)),
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    Unauthorized = 0x01,
    SubdomainTaken = 0x02,
    SubdomainNotReserved = 0x03,
    VersionIncompatible = 0x04,
    ServerFull = 0x05,
    Malformed = 0x06,
}

impl RejectReason {
    pub fn from_u8(v: u8) -> Result<Self, FrameError> {
        Ok(match v {
            0x01 => RejectReason::Unauthorized,
            0x02 => RejectReason::SubdomainTaken,
            0x03 => RejectReason::SubdomainNotReserved,
            0x04 => RejectReason::VersionIncompatible,
            0x05 => RejectReason::ServerFull,
            0x06 => RejectReason::Malformed,
            other => return Err(FrameError::UnknownRejectReason(other)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub ty: FrameType,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(ty: FrameType, payload: impl Into<Bytes>) -> Self {
        Self { ty, payload: payload.into() }
    }

    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, FrameError> {
        let ty_byte = reader.read_u8().await?;
        let ty = FrameType::from_u8(ty_byte)?;
        let len = reader.read_u32().await?;
        if len > MAX_FRAME_PAYLOAD {
            return Err(FrameError::OversizedPayload(len));
        }
        let mut buf = BytesMut::with_capacity(len as usize);
        buf.resize(len as usize, 0);
        reader.read_exact(&mut buf).await?;
        Ok(Self { ty, payload: buf.freeze() })
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> Result<(), FrameError> {
        let len: u32 = self.payload.len().try_into().map_err(|_| {
            FrameError::OversizedPayload(self.payload.len() as u32)
        })?;
        if len > MAX_FRAME_PAYLOAD {
            return Err(FrameError::OversizedPayload(len));
        }
        let mut header = [0u8; 5];
        header[0] = self.ty as u8;
        header[1..].copy_from_slice(&len.to_be_bytes());
        writer.write_all(&header).await?;
        writer.write_all(&self.payload).await?;
        writer.flush().await?;
        Ok(())
    }
}

/// Helper: write a length-prefixed (u8) string slice.
pub fn put_short_str(buf: &mut BytesMut, s: &str) -> Result<(), FrameError> {
    let len: u8 = s.len().try_into().map_err(|_| FrameError::StringTooLong(s.len()))?;
    buf.put_u8(len);
    buf.put_slice(s.as_bytes());
    Ok(())
}

/// Helper: read a length-prefixed (u8) UTF-8 string.
pub fn take_short_str(buf: &mut Bytes) -> Result<String, FrameError> {
    if buf.is_empty() {
        return Err(FrameError::Truncated);
    }
    let len = buf.get_u8() as usize;
    if buf.remaining() < len {
        return Err(FrameError::Truncated);
    }
    let bytes = buf.split_to(len);
    String::from_utf8(bytes.to_vec()).map_err(|_| FrameError::InvalidUtf8)
}

/// Helper: write a length-prefixed (u16) byte slice.
pub fn put_medium_bytes(buf: &mut BytesMut, b: &[u8]) -> Result<(), FrameError> {
    let len: u16 = b.len().try_into().map_err(|_| FrameError::StringTooLong(b.len()))?;
    buf.put_u16(len);
    buf.put_slice(b);
    Ok(())
}

/// Helper: read a length-prefixed (u16) byte slice.
pub fn take_medium_bytes(buf: &mut Bytes) -> Result<Bytes, FrameError> {
    if buf.remaining() < 2 {
        return Err(FrameError::Truncated);
    }
    let len = buf.get_u16() as usize;
    if buf.remaining() < len {
        return Err(FrameError::Truncated);
    }
    Ok(buf.split_to(len))
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown frame type: 0x{0:02x}")]
    UnknownType(u8),
    #[error("unknown reject reason: 0x{0:02x}")]
    UnknownRejectReason(u8),
    #[error("frame payload too large: {0} bytes")]
    OversizedPayload(u32),
    #[error("string too long: {0} bytes")]
    StringTooLong(usize),
    #[error("truncated frame payload")]
    Truncated,
    #[error("invalid utf-8 in frame payload")]
    InvalidUtf8,
    #[error("unexpected frame type {got:?}, expected {expected:?}")]
    Unexpected { expected: FrameType, got: FrameType },
}
