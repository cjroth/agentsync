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
    /// Hub → peer, first message in the handshake. Carries the vault id (so
    /// fresh-clone peers can discover it), the hub's identity pubkey, the
    /// hub-side nonce, and — in Phase 2+ — the SHA-256 fingerprint of the
    /// hub's TLS cert (empty in Phase 1).
    #[serde(rename = "hello_hub")]
    HelloHub {
        vault_id: String,
        #[serde(with = "serde_bytes")]
        hub_identity_pubkey: Vec<u8>,
        #[serde(with = "serde_bytes")]
        hub_nonce: Vec<u8>,
        #[serde(with = "serde_bytes")]
        tls_cert_fingerprint: Vec<u8>,
    },
    /// Peer → hub, second handshake message.
    #[serde(rename = "hello_peer")]
    HelloPeer {
        #[serde(with = "serde_bytes")]
        peer_identity_pubkey: Vec<u8>,
        #[serde(with = "serde_bytes")]
        peer_nonce: Vec<u8>,
        op: HelloOp,
    },
    /// Hub → peer, third handshake message: signature over the transcript.
    #[serde(rename = "proof_hub")]
    ProofHub {
        #[serde(with = "serde_bytes")]
        sig: Vec<u8>,
    },
    /// Peer → hub, fourth handshake message: signature over the transcript.
    #[serde(rename = "proof_peer")]
    ProofPeer {
        #[serde(with = "serde_bytes")]
        sig: Vec<u8>,
    },
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
