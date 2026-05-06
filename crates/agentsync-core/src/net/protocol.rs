use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HelloOp {
    Join,
    Create,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Frame {
    /// Client → server handshake.
    #[serde(rename = "hello")]
    Hello {
        vault_id: String,
        #[serde(with = "serde_bytes")]
        auth_token: Vec<u8>,
        op: HelloOp,
    },
    /// Server → client acknowledgement.
    #[serde(rename = "hello_ack")]
    HelloAck { vault_id: String },
    /// Opaque Automerge sync message.
    #[serde(rename = "sync")]
    Sync {
        #[serde(with = "serde_bytes")]
        bytes: Vec<u8>,
    },
    /// Request a blob by hash.
    #[serde(rename = "blob_fetch")]
    BlobFetch { hash: String },
    /// Send a blob.
    #[serde(rename = "blob_push")]
    BlobPush {
        hash: String,
        #[serde(with = "serde_bytes")]
        bytes: Vec<u8>,
    },
    #[serde(rename = "ping")]
    Ping { ts: i64 },
    #[serde(rename = "pong")]
    Pong { ts: i64 },
    #[serde(rename = "error")]
    Error { message: String },
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(rmp_serde::to_vec_named(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|e| Error::Protocol(format!("decode: {}", e)))
    }
}
