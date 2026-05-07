//! Handshake transcript construction and nonce helpers.
//!
//! Per AUTH.md, both sides of the handshake sign the same transcript:
//!
//! ```text
//! transcript =
//!     "agentsync-auth-v1"
//!     || hub_nonce || peer_nonce
//!     || tls_cert_fingerprint
//!     || hub_identity_pubkey || peer_identity_pubkey
//! ```
//!
//! The leading byte string is a domain-separation tag, not a negotiated
//! version — future handshake changes are a coordinated break.

use rand_core::{OsRng, RngCore};

pub const NONCE_LEN: usize = 32;
pub const HANDSHAKE_DOMAIN: &[u8] = b"agentsync-auth-v1";

pub fn random_nonce() -> [u8; NONCE_LEN] {
    let mut buf = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// Concatenate the handshake transcript exactly as both sides will sign it.
pub fn build_transcript(
    hub_nonce: &[u8; NONCE_LEN],
    peer_nonce: &[u8; NONCE_LEN],
    tls_cert_fingerprint: &[u8],
    hub_pubkey: &[u8; 32],
    peer_pubkey: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        HANDSHAKE_DOMAIN.len() + 2 * NONCE_LEN + tls_cert_fingerprint.len() + 64,
    );
    out.extend_from_slice(HANDSHAKE_DOMAIN);
    out.extend_from_slice(hub_nonce);
    out.extend_from_slice(peer_nonce);
    out.extend_from_slice(tls_cert_fingerprint);
    out.extend_from_slice(hub_pubkey);
    out.extend_from_slice(peer_pubkey);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_is_deterministic() {
        let hub_n = [1u8; NONCE_LEN];
        let peer_n = [2u8; NONCE_LEN];
        let hub_pk = [3u8; 32];
        let peer_pk = [4u8; 32];
        let a = build_transcript(&hub_n, &peer_n, &[], &hub_pk, &peer_pk);
        let b = build_transcript(&hub_n, &peer_n, &[], &hub_pk, &peer_pk);
        assert_eq!(a, b);
    }

    #[test]
    fn transcript_changes_with_inputs() {
        let n0 = [0u8; NONCE_LEN];
        let n1 = [1u8; NONCE_LEN];
        let pk = [0u8; 32];
        let a = build_transcript(&n0, &n0, &[], &pk, &pk);
        let b = build_transcript(&n1, &n0, &[], &pk, &pk);
        assert_ne!(a, b);
    }

    #[test]
    fn nonce_is_random() {
        let a = random_nonce();
        let b = random_nonce();
        assert_ne!(a, b);
    }
}
