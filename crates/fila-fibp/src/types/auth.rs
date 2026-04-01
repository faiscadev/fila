use bytes::Bytes;

use crate::error::FrameError;
use crate::error_code::ErrorCode;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::check_count;
use super::ProtocolMessage;

// ---------------------------------------------------------------------------
// Auth & ACL frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct CreateApiKeyResponse {
    pub error_code: ErrorCode,
    pub key_id: String,
    pub key: String,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct RevokeApiKeyRequest {
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct RevokeApiKeyResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct ListApiKeysRequest;

#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub key_id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct ListApiKeysResponse {
    pub error_code: ErrorCode,
    pub keys: Vec<ApiKeyInfo>,
}

#[derive(Debug, Clone)]
pub struct AclPermission {
    pub kind: String,
    pub pattern: String,
}

#[derive(Debug, Clone)]
pub struct SetAclRequest {
    pub key_id: String,
    pub permissions: Vec<AclPermission>,
}

#[derive(Debug, Clone)]
pub struct SetAclResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetAclRequest {
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct GetAclResponse {
    pub error_code: ErrorCode,
    pub key_id: String,
    pub is_superadmin: bool,
    pub permissions: Vec<AclPermission>,
}

// ---------------------------------------------------------------------------
// Encode/decode implementations
// ---------------------------------------------------------------------------

impl ProtocolMessage for CreateApiKeyRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.name);
        w.put_u64(self.expires_at_ms);
        w.put_bool(self.is_superadmin);
        RawFrame {
            opcode: Opcode::CreateApiKey as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let name = r.read_string()?;
        let expires_at_ms = r.read_u64()?;
        let is_superadmin = r.read_bool()?;
        Ok(Self {
            name,
            expires_at_ms,
            is_superadmin,
        })
    }
}

impl ProtocolMessage for CreateApiKeyResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.key_id);
        w.put_string(&self.key);
        w.put_bool(self.is_superadmin);
        RawFrame {
            opcode: Opcode::CreateApiKeyResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let key_id = r.read_string()?;
        let key = r.read_string()?;
        let is_superadmin = r.read_bool()?;
        Ok(Self {
            error_code,
            key_id,
            key,
            is_superadmin,
        })
    }
}

impl ProtocolMessage for RevokeApiKeyRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        RawFrame {
            opcode: Opcode::RevokeApiKey as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        Ok(Self { key_id })
    }
}

impl ProtocolMessage for RevokeApiKeyResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::RevokeApiKeyResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl ProtocolMessage for ListApiKeysRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        RawFrame {
            opcode: Opcode::ListApiKeys as u8,
            flags: 0,
            request_id,
            payload: Bytes::new(),
        }
    }

    fn decode(_payload: Bytes) -> Result<Self, FrameError> {
        Ok(Self)
    }
}

impl ProtocolMessage for ListApiKeysResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u16(self.keys.len() as u16);
        for k in &self.keys {
            w.put_string(&k.key_id);
            w.put_string(&k.name);
            w.put_u64(k.created_at_ms);
            w.put_u64(k.expires_at_ms);
            w.put_bool(k.is_superadmin);
        }
        RawFrame {
            opcode: Opcode::ListApiKeysResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            let key_id = r.read_string()?;
            let name = r.read_string()?;
            let created_at_ms = r.read_u64()?;
            let expires_at_ms = r.read_u64()?;
            let is_superadmin = r.read_bool()?;
            keys.push(ApiKeyInfo {
                key_id,
                name,
                created_at_ms,
                expires_at_ms,
                is_superadmin,
            });
        }
        Ok(Self { error_code, keys })
    }
}

impl ProtocolMessage for SetAclRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        w.put_u16(self.permissions.len() as u16);
        for p in &self.permissions {
            w.put_string(&p.kind);
            w.put_string(&p.pattern);
        }
        RawFrame {
            opcode: Opcode::SetAcl as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut permissions = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = r.read_string()?;
            let pattern = r.read_string()?;
            permissions.push(AclPermission { kind, pattern });
        }
        Ok(Self {
            key_id,
            permissions,
        })
    }
}

impl ProtocolMessage for SetAclResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::SetAclResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl ProtocolMessage for GetAclRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        RawFrame {
            opcode: Opcode::GetAcl as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        Ok(Self { key_id })
    }
}

impl ProtocolMessage for GetAclResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.key_id);
        w.put_bool(self.is_superadmin);
        w.put_u16(self.permissions.len() as u16);
        for p in &self.permissions {
            w.put_string(&p.kind);
            w.put_string(&p.pattern);
        }
        RawFrame {
            opcode: Opcode::GetAclResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let key_id = r.read_string()?;
        let is_superadmin = r.read_bool()?;
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut permissions = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = r.read_string()?;
            let pattern = r.read_string()?;
            permissions.push(AclPermission { kind, pattern });
        }
        Ok(Self {
            error_code,
            key_id,
            is_superadmin,
            permissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_api_key_round_trip() {
        let req = CreateApiKeyRequest {
            name: "my-key".to_string(),
            expires_at_ms: 1700000000000,
            is_superadmin: true,
        };
        let frame = req.encode(10);
        let decoded = CreateApiKeyRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.name, "my-key");
        assert_eq!(decoded.expires_at_ms, 1700000000000);
        assert!(decoded.is_superadmin);

        let resp = CreateApiKeyResponse {
            error_code: ErrorCode::Ok,
            key_id: "kid-1".to_string(),
            key: "secret-token".to_string(),
            is_superadmin: true,
        };
        let frame = resp.encode(10);
        let decoded = CreateApiKeyResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert_eq!(decoded.key, "secret-token");
        assert!(decoded.is_superadmin);
    }

    #[test]
    fn revoke_api_key_round_trip() {
        let req = RevokeApiKeyRequest {
            key_id: "kid-1".to_string(),
        };
        let frame = req.encode(11);
        let decoded = RevokeApiKeyRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");

        let resp = RevokeApiKeyResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(11);
        let decoded = RevokeApiKeyResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn list_api_keys_round_trip() {
        let req = ListApiKeysRequest;
        let frame = req.encode(12);
        ListApiKeysRequest::decode(frame.payload).unwrap();

        let resp = ListApiKeysResponse {
            error_code: ErrorCode::Ok,
            keys: vec![ApiKeyInfo {
                key_id: "kid-1".to_string(),
                name: "test-key".to_string(),
                created_at_ms: 1700000000000,
                expires_at_ms: 0,
                is_superadmin: false,
            }],
        };
        let frame = resp.encode(12);
        let decoded = ListApiKeysResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.keys.len(), 1);
        assert_eq!(decoded.keys[0].key_id, "kid-1");
        assert_eq!(decoded.keys[0].name, "test-key");
    }

    #[test]
    fn set_acl_round_trip() {
        let req = SetAclRequest {
            key_id: "kid-1".to_string(),
            permissions: vec![
                AclPermission {
                    kind: "produce".to_string(),
                    pattern: "orders.*".to_string(),
                },
                AclPermission {
                    kind: "admin".to_string(),
                    pattern: "*".to_string(),
                },
            ],
        };
        let frame = req.encode(13);
        let decoded = SetAclRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert_eq!(decoded.permissions.len(), 2);
        assert_eq!(decoded.permissions[0].kind, "produce");
        assert_eq!(decoded.permissions[0].pattern, "orders.*");

        let resp = SetAclResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(13);
        let decoded = SetAclResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn get_acl_round_trip() {
        let req = GetAclRequest {
            key_id: "kid-1".to_string(),
        };
        let frame = req.encode(14);
        let decoded = GetAclRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");

        let resp = GetAclResponse {
            error_code: ErrorCode::Ok,
            key_id: "kid-1".to_string(),
            is_superadmin: false,
            permissions: vec![AclPermission {
                kind: "consume".to_string(),
                pattern: "*".to_string(),
            }],
        };
        let frame = resp.encode(14);
        let decoded = GetAclResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert!(!decoded.is_superadmin);
        assert_eq!(decoded.permissions.len(), 1);
    }
}
