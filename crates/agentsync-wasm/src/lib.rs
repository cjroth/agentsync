//! WebAssembly bindings for `agentsync-core`.
//!
//! Exposes everything a JavaScript-side high-level Vault implementation
//! needs: identities & signing, the protocol Frame codec, the Automerge
//! Doc + sync state machine, label/restore APIs, authorized_keys parsing,
//! and handshake helpers.
//!
//! Networking, storage, and filesystem watching live in the JS layer (so
//! the same wasm runs in browsers, Node, Bun, Electron, Tauri, IDE
//! plugins, etc.) — see `sdks/typescript/src/vault.ts` for the high-level
//! API that orchestrates these primitives into a working sync client.

#![allow(clippy::new_without_default)]

use agentsync_core as core;
use core::doc::FileMeta;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

fn js_err<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

// ---- Identity ----

/// File-backed ed25519 identity. The JavaScript surface exposes generation,
/// seed import/export, signing, and pubkey access. The ssh-agent backend is
/// native-only and is not reachable from wasm.
#[wasm_bindgen]
pub struct Identity {
    inner: core::Identity,
}

#[wasm_bindgen]
impl Identity {
    /// Generate a fresh identity backed by a random ed25519 seed.
    #[wasm_bindgen(js_name = generate)]
    pub fn generate() -> Self {
        Self {
            inner: core::Identity::generate(),
        }
    }

    /// Import an identity from its 32-byte seed.
    #[wasm_bindgen(js_name = fromSeed)]
    pub fn from_seed(seed: &[u8]) -> Result<Identity, JsError> {
        if seed.len() != 32 {
            return Err(JsError::new("seed must be 32 bytes"));
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(seed);
        Ok(Self {
            inner: core::Identity::from_seed(buf),
        })
    }

    /// Export the 32-byte seed (file-backed identities only).
    #[wasm_bindgen]
    pub fn seed(&self) -> Result<Box<[u8]>, JsError> {
        Ok(self
            .inner
            .seed()
            .map_err(js_err)?
            .to_vec()
            .into_boxed_slice())
    }

    /// Public key of this identity.
    #[wasm_bindgen]
    pub fn pubkey(&self) -> Pubkey {
        Pubkey {
            inner: self.inner.pubkey(),
        }
    }

    /// Sign `message` and return the 64-byte ed25519 signature. Async to
    /// match the native signature; for file-backed identities completes
    /// synchronously.
    #[wasm_bindgen]
    pub async fn sign(&self, message: Box<[u8]>) -> Result<Box<[u8]>, JsError> {
        let sig = self.inner.sign(&message).await.map_err(js_err)?;
        Ok(sig.to_vec().into_boxed_slice())
    }
}

#[wasm_bindgen]
pub struct Pubkey {
    inner: core::Pubkey,
}

#[wasm_bindgen]
impl Pubkey {
    /// Construct a pubkey from raw 32 bytes.
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Pubkey, JsError> {
        Ok(Self {
            inner: core::Pubkey::from_bytes(bytes).map_err(js_err)?,
        })
    }

    /// Parse an `ssh-ed25519 <base64>` line.
    #[wasm_bindgen(js_name = fromSshString)]
    pub fn from_ssh_string(s: &str) -> Result<Pubkey, JsError> {
        Ok(Self {
            inner: core::Pubkey::from_ssh_string(s).map_err(js_err)?,
        })
    }

    /// Render the pubkey as `ssh-ed25519 <base64>`.
    #[wasm_bindgen(js_name = toSshString)]
    pub fn to_ssh_string(&self) -> String {
        self.inner.to_ssh_string()
    }

    /// SHA-256 fingerprint string in OpenSSH form: `SHA256:<base64>`.
    #[wasm_bindgen]
    pub fn fingerprint(&self) -> String {
        self.inner.fingerprint_sha256()
    }

    /// Raw 32-byte pubkey.
    #[wasm_bindgen]
    pub fn bytes(&self) -> Box<[u8]> {
        self.inner.as_bytes().to_vec().into_boxed_slice()
    }

    /// Verify a 64-byte signature over `message`.
    #[wasm_bindgen]
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        self.inner.verify(message, signature)
    }
}

// ---- authorized_keys ----

/// Parse an `authorized_keys` file body and return one entry per authorized
/// peer. Comments and blank lines are skipped. The result is a JS array of
/// `{ pubkey: string, label: string }` objects.
#[wasm_bindgen(js_name = parseAuthorizedKeys)]
pub fn parse_authorized_keys(body: &str) -> Result<JsValue, JsError> {
    let peers = core::parse_authorized_keys(body);
    let out: Vec<_> = peers
        .into_iter()
        .map(|p| AuthorizedPeerJson {
            pubkey: p.pubkey.to_ssh_string(),
            label: p.label,
        })
        .collect();
    serde_wasm_bindgen::to_value(&out).map_err(js_err)
}

/// Render a JS array of `{ pubkey, label }` entries as an `authorized_keys`
/// file body.
#[wasm_bindgen(js_name = renderAuthorizedKeys)]
pub fn render_authorized_keys(value: JsValue) -> Result<String, JsError> {
    let entries: Vec<AuthorizedPeerJson> = serde_wasm_bindgen::from_value(value).map_err(js_err)?;
    let peers: Vec<core::AuthorizedPeer> = entries
        .into_iter()
        .map(|e| {
            let pk = core::Pubkey::from_ssh_string(&e.pubkey).map_err(js_err)?;
            Ok::<_, JsError>(core::AuthorizedPeer {
                pubkey: pk,
                label: e.label,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(core::render_authorized_keys(&peers))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthorizedPeerJson {
    pubkey: String,
    label: String,
}

// ---- handshake helpers ----

/// 32-byte cryptographically secure random nonce, used for handshake transcripts.
#[wasm_bindgen(js_name = randomNonce)]
pub fn random_nonce() -> Box<[u8]> {
    core::random_nonce().to_vec().into_boxed_slice()
}

/// Build the canonical handshake transcript that both sides sign.
#[wasm_bindgen(js_name = buildTranscript)]
pub fn build_transcript(
    hub_nonce: &[u8],
    peer_nonce: &[u8],
    tls_cert_fingerprint: &[u8],
    hub_pubkey: &[u8],
    peer_pubkey: &[u8],
) -> Result<Box<[u8]>, JsError> {
    let hub_n: [u8; 32] = hub_nonce
        .try_into()
        .map_err(|_| JsError::new("hub_nonce must be 32 bytes"))?;
    let peer_n: [u8; 32] = peer_nonce
        .try_into()
        .map_err(|_| JsError::new("peer_nonce must be 32 bytes"))?;
    let hub_pk: [u8; 32] = hub_pubkey
        .try_into()
        .map_err(|_| JsError::new("hub_pubkey must be 32 bytes"))?;
    let peer_pk: [u8; 32] = peer_pubkey
        .try_into()
        .map_err(|_| JsError::new("peer_pubkey must be 32 bytes"))?;
    Ok(
        core::build_transcript(&hub_n, &peer_n, tls_cert_fingerprint, &hub_pk, &peer_pk)
            .into_boxed_slice(),
    )
}

// ---- Frame codec ----

/// Decode a msgpack-encoded protocol frame and return it as a JS object.
#[wasm_bindgen(js_name = decodeFrame)]
pub fn decode_frame(bytes: &[u8]) -> Result<JsValue, JsError> {
    let frame = core::Frame::decode(bytes).map_err(js_err)?;
    serde_wasm_bindgen::to_value(&frame).map_err(js_err)
}

/// Encode a JS-side frame object (matching the `Frame` enum shape) to
/// msgpack bytes.
#[wasm_bindgen(js_name = encodeFrame)]
pub fn encode_frame(value: JsValue) -> Result<Box<[u8]>, JsError> {
    let frame: core::Frame = serde_wasm_bindgen::from_value(value).map_err(js_err)?;
    Ok(frame.encode().map_err(js_err)?.into_boxed_slice())
}

// ---- Sync state ----

/// Per-peer sync state for the Automerge incremental sync protocol. One
/// `SyncState` per remote peer; encode/decode for persistence so reconnects
/// don't replay the entire history.
#[wasm_bindgen]
pub struct SyncState {
    inner: automerge::sync::State,
}

#[wasm_bindgen]
impl SyncState {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: automerge::sync::State::new(),
        }
    }

    /// Serialize the state for storage. Pair with [`SyncState::decode`].
    #[wasm_bindgen]
    pub fn encode(&self) -> Box<[u8]> {
        self.inner.encode().into_boxed_slice()
    }

    /// Restore a state from previously-encoded bytes.
    #[wasm_bindgen]
    pub fn decode(bytes: &[u8]) -> Result<SyncState, JsError> {
        Ok(Self {
            inner: automerge::sync::State::decode(bytes).map_err(js_err)?,
        })
    }
}

// ---- Doc / CRDT ----

/// Wraps an Automerge-backed agentsync document. Use [`new`] to create a
/// fresh vault doc, [`load`] to restore from saved bytes, and [`save`] to
/// serialize. Mutators apply Automerge changes locally; merge with a remote
/// peer's bytes via [`merge`] (full-doc) or via the sync-state pair
/// `generate_sync_message` / `receive_sync_message` (incremental).
#[wasm_bindgen]
pub struct Doc {
    inner: core::Doc,
}

#[wasm_bindgen]
impl Doc {
    /// Create a brand new vault document with the given vault id.
    #[wasm_bindgen(constructor)]
    pub fn new(vault_id: &str) -> Result<Doc, JsError> {
        Ok(Self {
            inner: core::Doc::new(vault_id).map_err(js_err)?,
        })
    }

    /// Load a saved vault document.
    #[wasm_bindgen]
    pub fn load(bytes: &[u8]) -> Result<Doc, JsError> {
        Ok(Self {
            inner: core::Doc::load(bytes).map_err(js_err)?,
        })
    }

    /// Serialize the document to bytes.
    #[wasm_bindgen]
    pub fn save(&mut self) -> Box<[u8]> {
        self.inner.save().into_boxed_slice()
    }

    /// Save only the changes since the last save.
    #[wasm_bindgen(js_name = saveIncremental)]
    pub fn save_incremental(&mut self) -> Box<[u8]> {
        self.inner.save_incremental().into_boxed_slice()
    }

    #[wasm_bindgen(js_name = vaultId)]
    pub fn vault_id(&mut self) -> Result<String, JsError> {
        self.inner.vault_id().map_err(js_err)
    }

    /// Current document heads (each 32 bytes). Useful for label snapshots
    /// and incremental sync state tracking.
    #[wasm_bindgen]
    pub fn heads(&mut self) -> Vec<js_sys::Uint8Array> {
        self.inner
            .heads()
            .iter()
            .map(|h| {
                let bytes: [u8; 32] = h.0;
                js_sys::Uint8Array::from(&bytes[..])
            })
            .collect()
    }

    /// Merge in changes from `other`. Returns true if the local doc changed.
    #[wasm_bindgen]
    pub fn merge(&mut self, other: &mut Doc) -> Result<bool, JsError> {
        self.inner.merge(&mut other.inner).map_err(js_err)
    }

    /// Generate the next outbound sync message for `state`. Returns
    /// `undefined` (mapped from `None`) when no message is needed.
    #[wasm_bindgen(js_name = generateSyncMessage)]
    pub fn generate_sync_message(&mut self, state: &mut SyncState) -> Option<Box<[u8]>> {
        self.inner
            .generate_sync_message(&mut state.inner)
            .map(|m| m.into_boxed_slice())
    }

    /// Apply an inbound sync message. Returns true if heads moved.
    #[wasm_bindgen(js_name = receiveSyncMessage)]
    pub fn receive_sync_message(
        &mut self,
        state: &mut SyncState,
        bytes: &[u8],
    ) -> Result<bool, JsError> {
        self.inner
            .receive_sync_message(&mut state.inner, bytes)
            .map_err(js_err)
    }

    /// Write a UTF-8 text file at `path`. Returns the stable file id.
    #[wasm_bindgen(js_name = writeTextFile)]
    pub fn write_text_file(&mut self, path: &str, content: &str) -> Result<String, JsError> {
        self.inner.write_text_file(path, content).map_err(js_err)
    }

    /// Read the UTF-8 text file at `path`.
    #[wasm_bindgen(js_name = readFile)]
    pub fn read_file(&mut self, path: &str) -> Result<String, JsError> {
        self.inner.read_file(path).map_err(js_err)
    }

    #[wasm_bindgen(js_name = fileExists)]
    pub fn file_exists(&mut self, path: &str) -> bool {
        self.inner.file_exists(path)
    }

    #[wasm_bindgen(js_name = deleteFile)]
    pub fn delete_file(&mut self, path: &str) -> Result<(), JsError> {
        self.inner.delete_file(path).map_err(js_err)
    }

    /// Rename a file. The stable file id is preserved.
    #[wasm_bindgen(js_name = renameFile)]
    pub fn rename_file(&mut self, from: &str, to: &str) -> Result<(), JsError> {
        self.inner.rename_file(from, to).map_err(js_err)
    }

    /// Write a binary attachment under `path`, content-addressed by `hash`
    /// (the JS side computes the SHA-256). Returns the file id.
    #[wasm_bindgen(js_name = writeAttachment)]
    pub fn write_attachment(
        &mut self,
        path: &str,
        hash: &str,
        size: i64,
    ) -> Result<String, JsError> {
        self.inner
            .write_attachment(path, hash, size)
            .map_err(js_err)
    }

    /// List all current files. Returns an array of `FileMeta` objects.
    #[wasm_bindgen(js_name = listFiles)]
    pub fn list_files(&mut self) -> Result<JsValue, JsError> {
        let files = self.inner.list_files().map_err(js_err)?;
        files_to_js(&files)
    }

    /// Create a directory at `path`. Returns the directory id.
    #[wasm_bindgen(js_name = createDirectory)]
    pub fn create_directory(&mut self, path: &str) -> Result<String, JsError> {
        self.inner.create_directory(path).map_err(js_err)
    }

    /// Delete a directory.
    #[wasm_bindgen(js_name = deleteDirectory)]
    pub fn delete_directory(&mut self, path: &str, recursive: bool) -> Result<(), JsError> {
        self.inner.delete_directory(path, recursive).map_err(js_err)
    }

    /// List directories.
    #[wasm_bindgen(js_name = listDirectories)]
    pub fn list_directories(&mut self) -> Result<JsValue, JsError> {
        let dirs = self.inner.list_directories().map_err(js_err)?;
        serde_wasm_bindgen::to_value(&dirs).map_err(js_err)
    }

    // ---- History / labels / restore ----

    /// Create a named snapshot of the current heads.
    #[wasm_bindgen(js_name = createLabel)]
    pub fn create_label(&mut self, name: &str) -> Result<(), JsError> {
        self.inner.create_label(name).map_err(js_err)
    }

    /// Delete a label.
    #[wasm_bindgen(js_name = deleteLabel)]
    pub fn delete_label(&mut self, name: &str) -> Result<(), JsError> {
        self.inner.delete_label(name).map_err(js_err)
    }

    /// List all labels with their heads + creation timestamps.
    #[wasm_bindgen(js_name = listLabels)]
    pub fn list_labels(&mut self) -> Result<JsValue, JsError> {
        let labels = self.inner.list_labels().map_err(js_err)?;
        let out: Vec<_> = labels
            .into_iter()
            .map(|l| LabelJson {
                name: l.name,
                heads_b64: encode_heads_b64(&l.heads),
                created_at_ms: l.created_at,
            })
            .collect();
        serde_wasm_bindgen::to_value(&out).map_err(js_err)
    }

    /// Restore the doc to the snapshot named `label`. Additive: produces
    /// new forward-going changes that recreate the past state, preserving
    /// any concurrent changes from other peers.
    #[wasm_bindgen(js_name = restoreToLabel)]
    pub fn restore_to_label(&mut self, label: &str) -> Result<(), JsError> {
        let heads = self.inner.get_label_heads(label).map_err(js_err)?;
        self.inner.restore_to_heads(&heads).map_err(js_err)
    }

    /// Restore the doc to the state at `target_ms` (Unix milliseconds).
    /// Convenience wrapper over `heads_at_time` + `restore_to_heads`.
    /// Takes `f64` so JS callers can pass `Date.now()` directly without
    /// having to convert to BigInt; agentsync timestamps fit comfortably
    /// in IEEE-754 precision.
    #[wasm_bindgen(js_name = restoreToTime)]
    pub fn restore_to_time(&mut self, target_ms: f64) -> Result<(), JsError> {
        self.inner.restore_to_time(target_ms as i64).map_err(js_err)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LabelJson {
    name: String,
    heads_b64: String,
    created_at_ms: i64,
}

fn encode_heads_b64(heads: &[automerge::ChangeHash]) -> String {
    use base64::Engine;
    let mut bytes = Vec::with_capacity(heads.len() * 32);
    for h in heads {
        bytes.extend_from_slice(&h.0);
    }
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

fn files_to_js(files: &[FileMeta]) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(files).map_err(js_err)
}

// ---- Helpers ----

/// SHA-256 of arbitrary bytes, hex-encoded. Matches the on-disk content
/// hash format used by agentsync.
#[wasm_bindgen(js_name = contentHash)]
pub fn content_hash(bytes: &[u8]) -> String {
    core::content_hash(bytes)
}

/// Schema version of vault documents produced by this SDK.
#[wasm_bindgen(js_name = schemaVersion)]
pub fn schema_version() -> u32 {
    core::SCHEMA_VERSION as u32
}

/// Default rendezvous port (1234).
#[wasm_bindgen(js_name = defaultPort)]
pub fn default_port() -> u16 {
    core::DEFAULT_PORT
}

/// Normalize a rendezvous URL (appends the default port when missing).
#[wasm_bindgen(js_name = normalizeRendezvousUrl)]
pub fn normalize_rendezvous_url(url: &str) -> String {
    core::normalize_rendezvous_url(url)
}
