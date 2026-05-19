//! High-level message types over the control channel.

use bytes::{Buf, BytesMut};

use crate::frame::{
    put_medium_bytes, put_short_str, take_medium_bytes, take_short_str, Frame, FrameError,
    FrameType, RejectReason,
};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone)]
pub struct Hello {
    pub version: u16,
    pub subdomain: String,
    pub auth_token: Vec<u8>,
}

impl Hello {
    pub fn into_frame(self) -> Result<Frame, FrameError> {
        let mut buf = BytesMut::with_capacity(2 + 1 + self.subdomain.len() + 2 + self.auth_token.len());
        buf.extend_from_slice(&self.version.to_be_bytes());
        put_short_str(&mut buf, &self.subdomain)?;
        put_medium_bytes(&mut buf, &self.auth_token)?;
        Ok(Frame::new(FrameType::Hello, buf.freeze()))
    }

    pub fn from_frame(frame: Frame) -> Result<Self, FrameError> {
        if frame.ty != FrameType::Hello {
            return Err(FrameError::Unexpected { expected: FrameType::Hello, got: frame.ty });
        }
        let mut buf = frame.payload;
        if buf.remaining() < 2 {
            return Err(FrameError::Truncated);
        }
        let version = buf.get_u16();
        let subdomain = take_short_str(&mut buf)?;
        let auth_token = take_medium_bytes(&mut buf)?.to_vec();
        Ok(Self { version, subdomain, auth_token })
    }
}

#[derive(Debug, Clone)]
pub struct Accept {
    pub server_version: u16,
    pub assigned_subdomain: String,
}

impl Accept {
    pub fn into_frame(self) -> Result<Frame, FrameError> {
        let mut buf = BytesMut::with_capacity(2 + 1 + self.assigned_subdomain.len());
        buf.extend_from_slice(&self.server_version.to_be_bytes());
        put_short_str(&mut buf, &self.assigned_subdomain)?;
        Ok(Frame::new(FrameType::Accept, buf.freeze()))
    }

    pub fn from_frame(frame: Frame) -> Result<Self, FrameError> {
        if frame.ty != FrameType::Accept {
            return Err(FrameError::Unexpected { expected: FrameType::Accept, got: frame.ty });
        }
        let mut buf = frame.payload;
        if buf.remaining() < 2 {
            return Err(FrameError::Truncated);
        }
        let server_version = buf.get_u16();
        let assigned_subdomain = take_short_str(&mut buf)?;
        Ok(Self { server_version, assigned_subdomain })
    }
}

#[derive(Debug, Clone)]
pub struct Reject {
    pub reason: RejectReason,
    pub message: String,
}

impl Reject {
    pub fn into_frame(self) -> Result<Frame, FrameError> {
        let mut buf = BytesMut::with_capacity(1 + 2 + self.message.len());
        buf.extend_from_slice(&[self.reason as u8]);
        put_medium_bytes(&mut buf, self.message.as_bytes())?;
        Ok(Frame::new(FrameType::Reject, buf.freeze()))
    }

    pub fn from_frame(frame: Frame) -> Result<Self, FrameError> {
        if frame.ty != FrameType::Reject {
            return Err(FrameError::Unexpected { expected: FrameType::Reject, got: frame.ty });
        }
        let mut buf = frame.payload;
        if buf.remaining() < 1 {
            return Err(FrameError::Truncated);
        }
        let reason = RejectReason::from_u8(buf.get_u8())?;
        let msg_bytes = take_medium_bytes(&mut buf)?;
        let message = String::from_utf8(msg_bytes.to_vec()).map_err(|_| FrameError::InvalidUtf8)?;
        Ok(Self { reason, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trip() {
        let h = Hello {
            version: PROTOCOL_VERSION,
            subdomain: "acme".into(),
            auth_token: b"some-token-bytes".to_vec(),
        };
        let frame = h.clone().into_frame().unwrap();
        let parsed = Hello::from_frame(frame).unwrap();
        assert_eq!(parsed.version, h.version);
        assert_eq!(parsed.subdomain, h.subdomain);
        assert_eq!(parsed.auth_token, h.auth_token);
    }

    #[test]
    fn accept_round_trip() {
        let a = Accept { server_version: 1, assigned_subdomain: "acme".into() };
        let frame = a.clone().into_frame().unwrap();
        let parsed = Accept::from_frame(frame).unwrap();
        assert_eq!(parsed.server_version, a.server_version);
        assert_eq!(parsed.assigned_subdomain, a.assigned_subdomain);
    }

    #[test]
    fn reject_round_trip() {
        let r = Reject { reason: RejectReason::Unauthorized, message: "bad token".into() };
        let frame = r.clone().into_frame().unwrap();
        let parsed = Reject::from_frame(frame).unwrap();
        assert!(matches!(parsed.reason, RejectReason::Unauthorized));
        assert_eq!(parsed.message, "bad token");
    }
}
