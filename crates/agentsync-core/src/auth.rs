use crate::error::{Error, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

pub const VAULT_KEY_LEN: usize = 32;
pub const AUTH_TOKEN_LEN: usize = 32;
const AUTH_TOKEN_INFO: &[u8] = b"agentsync-auth-v1";

pub type VaultKey = [u8; VAULT_KEY_LEN];

pub fn generate_vault_key() -> VaultKey {
    let mut key = [0u8; VAULT_KEY_LEN];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

pub fn derive_auth_token(vault_key: &VaultKey) -> [u8; AUTH_TOKEN_LEN] {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(vault_key).expect("HMAC key length is fixed");
    mac.update(AUTH_TOKEN_INFO);
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; AUTH_TOKEN_LEN];
    out.copy_from_slice(&bytes);
    out
}

pub fn verify_auth_token(vault_key: &VaultKey, presented: &[u8]) -> bool {
    let expected = derive_auth_token(vault_key);
    constant_time_eq(&expected, presented)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn encode_key(key: &VaultKey) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(key)
}

pub fn decode_key(s: &str) -> Result<VaultKey> {
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s.trim())
        .map_err(|e| Error::Auth(format!("invalid base64 key: {}", e)))?;
    if bytes.len() != VAULT_KEY_LEN {
        return Err(Error::Auth(format!(
            "expected {}-byte key, got {}",
            VAULT_KEY_LEN,
            bytes.len()
        )));
    }
    let mut out = [0u8; VAULT_KEY_LEN];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_deterministic() {
        let k = [1u8; VAULT_KEY_LEN];
        assert_eq!(derive_auth_token(&k), derive_auth_token(&k));
    }

    #[test]
    fn token_differs_per_key() {
        let a = [1u8; VAULT_KEY_LEN];
        let b = [2u8; VAULT_KEY_LEN];
        assert_ne!(derive_auth_token(&a), derive_auth_token(&b));
    }

    #[test]
    fn verify_round_trip() {
        let k = generate_vault_key();
        let t = derive_auth_token(&k);
        assert!(verify_auth_token(&k, &t));
        assert!(!verify_auth_token(&k, &[0u8; AUTH_TOKEN_LEN]));
    }

    #[test]
    fn key_codec_round_trip() {
        let k = generate_vault_key();
        let s = encode_key(&k);
        let k2 = decode_key(&s).unwrap();
        assert_eq!(k, k2);
    }
}
