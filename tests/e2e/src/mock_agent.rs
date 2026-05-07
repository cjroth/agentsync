//! In-process ssh-agent that speaks just enough of the protocol to support
//! agentsync's signing path: REQUEST_IDENTITIES (11) and SIGN_REQUEST (13)
//! for ed25519 keys. Lets us exercise the agent backend hermetically without
//! requiring a system `ssh-agent` to be running.

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
const SSH_AGENTC_SIGN_REQUEST: u8 = 13;
const SSH_AGENT_SIGN_RESPONSE: u8 = 14;
const SSH_AGENT_FAILURE: u8 = 5;
const SSH_KEY_TYPE: &[u8] = b"ssh-ed25519";

/// A fake ssh-agent listening on a per-test Unix socket.
pub struct MockAgent {
    pub socket_path: PathBuf,
    pub signing: SigningKey,
    _dir: TempDir,
}

impl MockAgent {
    /// Spawn a mock agent in the background. Holds a single ed25519 key.
    pub async fn start(signing: SigningKey) -> Result<Self> {
        let dir = TempDir::new()?;
        let socket_path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let key = Arc::new(signing.clone());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(t) => t,
                    Err(_) => return,
                };
                let key = key.clone();
                tokio::spawn(async move {
                    loop {
                        let mut len_buf = [0u8; 4];
                        if stream.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut payload = vec![0u8; len];
                        if stream.read_exact(&mut payload).await.is_err() {
                            return;
                        }
                        if payload.is_empty() {
                            return;
                        }
                        let resp = handle_request(&key, &payload);
                        let mut framed = Vec::with_capacity(4 + resp.len());
                        framed.extend_from_slice(&(resp.len() as u32).to_be_bytes());
                        framed.extend_from_slice(&resp);
                        if stream.write_all(&framed).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Ok(Self {
            socket_path,
            signing,
            _dir: dir,
        })
    }
}

fn handle_request(key: &SigningKey, payload: &[u8]) -> Vec<u8> {
    match payload[0] {
        SSH_AGENTC_REQUEST_IDENTITIES => {
            // Respond with one ed25519 identity.
            let pub_blob = encode_pub_blob(&key.verifying_key().to_bytes());
            let comment = b"mock";
            let mut out = vec![SSH_AGENT_IDENTITIES_ANSWER];
            out.extend_from_slice(&(1u32).to_be_bytes()); // 1 identity
            write_string(&mut out, &pub_blob);
            write_string(&mut out, comment);
            out
        }
        SSH_AGENTC_SIGN_REQUEST => {
            let mut cursor = 1usize;
            let _key_blob = match read_string(payload, &mut cursor) {
                Some(s) => s,
                None => return vec![SSH_AGENT_FAILURE],
            };
            let data = match read_string(payload, &mut cursor) {
                Some(s) => s,
                None => return vec![SSH_AGENT_FAILURE],
            };
            // 4-byte flags follow but we don't honor them.
            let signature = key.sign(data).to_bytes();
            // sig_blob = string("ssh-ed25519") || string(<sig>)
            let mut sig_blob = Vec::new();
            write_string(&mut sig_blob, SSH_KEY_TYPE);
            write_string(&mut sig_blob, &signature);
            let mut out = vec![SSH_AGENT_SIGN_RESPONSE];
            write_string(&mut out, &sig_blob);
            out
        }
        _ => vec![SSH_AGENT_FAILURE],
    }
}

fn encode_pub_blob(pubkey: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::new();
    write_string(&mut out, SSH_KEY_TYPE);
    write_string(&mut out, pubkey);
    out
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

fn read_string<'a>(buf: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    if buf.len() < *cursor + 4 {
        return None;
    }
    let len = u32::from_be_bytes([
        buf[*cursor],
        buf[*cursor + 1],
        buf[*cursor + 2],
        buf[*cursor + 3],
    ]) as usize;
    *cursor += 4;
    if buf.len() < *cursor + len {
        return None;
    }
    let s = &buf[*cursor..*cursor + len];
    *cursor += len;
    Some(s)
}
