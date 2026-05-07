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
    let trimmed = s.trim();
    if looks_like_uuid(trimmed) {
        return Err(Error::Auth(format!(
            "expected base64-encoded {}-byte vault key (~{} chars), got a UUID. \
             Did you confuse vault_id and vault_key? \
             Use the `vault_key` value from `agentsync init`, not `vault_id`.",
            VAULT_KEY_LEN,
            base64_len_no_pad(VAULT_KEY_LEN),
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(trimmed)
        .map_err(|e| {
            Error::Auth(format!(
                "invalid vault key: expected base64-encoded {}-byte key (~{} chars). \
                 Pass the `vault_key` value from `agentsync init`. \
                 (decoder error: {})",
                VAULT_KEY_LEN,
                base64_len_no_pad(VAULT_KEY_LEN),
                e
            ))
        })?;
    if bytes.len() != VAULT_KEY_LEN {
        return Err(Error::Auth(format!(
            "vault key has wrong length: expected {} bytes (~{} base64 chars), got {} bytes. \
             Pass the `vault_key` value from `agentsync init`.",
            VAULT_KEY_LEN,
            base64_len_no_pad(VAULT_KEY_LEN),
            bytes.len()
        )));
    }
    let mut out = [0u8; VAULT_KEY_LEN];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn looks_like_uuid(s: &str) -> bool {
    // 8-4-4-4-12 hex with hyphens.
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let is_hyphen = matches!(i, 8 | 13 | 18 | 23);
        if is_hyphen {
            if c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

const fn base64_len_no_pad(n: usize) -> usize {
    (n * 4 + 2) / 3
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
