//! Automerge schema and document operations.
//!
//! A vault is a single Automerge document with this shape:
//! ```text
//! root: {
//!     schema_version: u64,
//!     vault_id: String,
//!     directories: Map<dir_id, DirectoryMeta>,
//!     files:       Map<file_id, FileEntry>,
//!     labels:      Map<label_name, encoded_heads_bytes>,
//! }
//! ```
//!
//! Files and directories are keyed by stable UUIDs; paths are mutable fields.

use crate::error::{Error, Result};
use automerge::transaction::{CommitOptions, Transactable};
use automerge::{
    ActorId, AutoCommit, ChangeHash, ObjId, ObjType, ReadDoc, ScalarValue, Value, ROOT,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub mod files;
pub mod directories;
pub mod history;

pub const SCHEMA_VERSION: i64 = 1;

pub type FileId = String;
pub type DirId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    Text,
    Attachment,
}

impl FileKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileKind::Text => "text",
            FileKind::Attachment => "attachment",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "text" => Ok(FileKind::Text),
            "attachment" => Ok(FileKind::Attachment),
            other => Err(Error::Other(format!("unknown file kind: {}", other))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub id: FileId,
    pub path: String,
    pub kind: FileKind,
    pub size: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub binary_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryMeta {
    pub id: DirId,
    pub path: String,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub heads: Vec<ChangeHash>,
    pub created_at: i64,
}

/// Wrapper around an Automerge document that enforces the agentsync schema.
pub struct Doc {
    pub(crate) inner: AutoCommit,
}

impl Doc {
    /// Create a fresh empty vault document with `vault_id` baked in.
    ///
    /// The schema-init change is committed with a deterministic actor id and
    /// timestamp derived from `vault_id`, so any peer that calls `Doc::new`
    /// before syncing produces an identical change hash. That keeps the
    /// `root.{files,directories,labels}` ObjIds aligned across peers and
    /// prevents the merged doc from carrying two conflicting versions of each
    /// root map.
    pub fn new(vault_id: &str) -> Result<Self> {
        let genesis_actor = genesis_actor(vault_id);
        let mut doc = AutoCommit::new();
        doc.set_actor(genesis_actor);
        doc.put(ROOT, "schema_version", SCHEMA_VERSION)?;
        doc.put(ROOT, "vault_id", vault_id)?;
        doc.put_object(ROOT, "directories", ObjType::Map)?;
        doc.put_object(ROOT, "files", ObjType::Map)?;
        doc.put_object(ROOT, "labels", ObjType::Map)?;
        doc.commit_with(CommitOptions::default().with_time(0));
        // After genesis, switch to a unique actor so subsequent writes can
        // be distinguished and do not share a logical-clock with other peers.
        doc.set_actor(ActorId::random());
        Ok(Doc { inner: doc })
    }

    pub fn load(bytes: &[u8]) -> Result<Self> {
        let inner = AutoCommit::load(bytes)?;
        Ok(Doc { inner })
    }

    pub fn save(&mut self) -> Vec<u8> {
        self.inner.save()
    }

    pub fn save_incremental(&mut self) -> Vec<u8> {
        self.inner.save_incremental()
    }

    pub fn fork(&mut self) -> Self {
        Doc {
            inner: self.inner.fork(),
        }
    }

    pub fn vault_id(&mut self) -> Result<String> {
        match self.inner.get(ROOT, "vault_id")? {
            Some((Value::Scalar(s), _)) => match s.as_ref() {
                ScalarValue::Str(v) => Ok(v.to_string()),
                other => Err(Error::Other(format!(
                    "vault_id is not a string: {:?}",
                    other
                ))),
            },
            _ => Err(Error::Other("vault_id missing".into())),
        }
    }

    pub fn heads(&mut self) -> Vec<ChangeHash> {
        self.inner.get_heads()
    }

    pub(crate) fn map_obj(&mut self, key: &str) -> Result<ObjId> {
        match self.inner.get(ROOT, key)? {
            Some((Value::Object(_), id)) => Ok(id),
            _ => Err(Error::Other(format!(
                "schema missing root.{}",
                key
            ))),
        }
    }

    pub(crate) fn files_obj(&mut self) -> Result<ObjId> {
        self.map_obj("files")
    }

    pub(crate) fn directories_obj(&mut self) -> Result<ObjId> {
        self.map_obj("directories")
    }

    pub(crate) fn labels_obj(&mut self) -> Result<ObjId> {
        self.map_obj("labels")
    }

    /// Apply incoming sync changes by saving + reloading. Returns true if anything changed.
    pub fn merge(&mut self, other: &mut Doc) -> Result<bool> {
        let before = self.inner.get_heads();
        self.inner.merge(&mut other.inner)?;
        let after = self.inner.get_heads();
        Ok(before != after)
    }
}

fn genesis_actor(vault_id: &str) -> ActorId {
    let mut hasher = Sha256::new();
    hasher.update(b"agentsync-genesis-actor-v1");
    hasher.update(vault_id.as_bytes());
    let digest = hasher.finalize();
    ActorId::from(&digest[..])
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// On `wasm32-unknown-unknown` there is no `SystemTime`. Use the JS host's
/// `Date.now()` as the wall-clock source.
#[cfg(target_arch = "wasm32")]
pub(crate) fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

impl Doc {
    /// Commit the current transaction stamping it with wall-clock time.
    /// The default `AutoCommit::commit()` records `time = None` (serialized
    /// as 0) which makes time-based history queries (`restore-at`) useless,
    /// so all schema mutators in this crate go through here.
    pub(crate) fn commit_now(&mut self) {
        self.inner
            .commit_with(CommitOptions::default().with_time(now_ms()));
    }
}

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Compute the SHA-256 hex digest of arbitrary bytes.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Read a string scalar at `obj.key`, returning None if missing.
pub(crate) fn get_str(doc: &mut impl ReadDoc, obj: &ObjId, key: &str) -> Result<Option<String>> {
    match doc.get(obj, key)? {
        Some((Value::Scalar(s), _)) => match s.as_ref() {
            ScalarValue::Str(v) => Ok(Some(v.to_string())),
            ScalarValue::Null => Ok(None),
            other => Err(Error::Other(format!(
                "expected string at {}, got {:?}",
                key, other
            ))),
        },
        _ => Ok(None),
    }
}

pub(crate) fn get_int(doc: &mut impl ReadDoc, obj: &ObjId, key: &str) -> Result<Option<i64>> {
    match doc.get(obj, key)? {
        Some((Value::Scalar(s), _)) => match s.as_ref() {
            ScalarValue::Int(v) => Ok(Some(*v)),
            ScalarValue::Uint(v) => Ok(Some(*v as i64)),
            ScalarValue::Null => Ok(None),
            other => Err(Error::Other(format!(
                "expected int at {}, got {:?}",
                key, other
            ))),
        },
        _ => Ok(None),
    }
}

pub(crate) fn get_text(doc: &mut impl ReadDoc, obj: &ObjId, key: &str) -> Result<Option<(ObjId, String)>> {
    match doc.get(obj, key)? {
        Some((Value::Object(ObjType::Text), id)) => {
            let text = doc.text(&id)?;
            Ok(Some((id, text)))
        }
        _ => Ok(None),
    }
}

pub(crate) fn get_object(doc: &mut impl ReadDoc, obj: &ObjId, key: &str) -> Result<Option<ObjId>> {
    match doc.get(obj, key)? {
        Some((Value::Object(_), id)) => Ok(Some(id)),
        _ => Ok(None),
    }
}

/// Iterate keys of a map object.
pub(crate) fn map_keys(doc: &mut impl ReadDoc, obj: &ObjId) -> Vec<String> {
    doc.keys(obj).collect()
}

pub use history::*;
