use crate::doc::Label;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use base64::Engine;
use automerge::ChangeHash;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub label: String,
    pub heads: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotIndexFile {
    pub schema_version: i64,
    pub labels: Vec<SnapshotEntry>,
}

/// Local cache of the labels inside the Automerge doc, persisted as JSON.
pub struct SnapshotIndex {
    root: PathBuf,
}

impl SnapshotIndex {
    pub fn new(storage_root: impl AsRef<Path>) -> Self {
        Self {
            root: storage_root.as_ref().join("snapshots"),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    pub async fn write(&self, labels: &[Label]) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        let entries: Vec<SnapshotEntry> = labels
            .iter()
            .map(|l| {
                let mut bytes = Vec::with_capacity(l.heads.len() * 32);
                for h in &l.heads {
                    bytes.extend_from_slice(h.as_ref());
                }
                SnapshotEntry {
                    label: l.name.clone(),
                    heads: base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
                    created_at: l.created_at,
                }
            })
            .collect();
        let file = SnapshotIndexFile {
            schema_version: 1,
            labels: entries,
        };
        let json = serde_json::to_string_pretty(&file)?;
        let tmp = self.root.join(".index.json.tmp");
        fs::write(&tmp, json.as_bytes()).await?;
        fs::rename(&tmp, self.path()).await?;
        Ok(())
    }

    pub async fn read(&self) -> Result<Vec<SnapshotEntry>> {
        match fs::read(self.path()).await {
            Ok(bytes) => {
                let f: SnapshotIndexFile = serde_json::from_slice(&bytes)?;
                Ok(f.labels)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }
}

pub fn decode_b64_heads(s: &str) -> Result<Vec<ChangeHash>> {
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s)
        .map_err(|e| crate::error::Error::Other(format!("base64: {}", e)))?;
    if bytes.len() % 32 != 0 {
        return Err(crate::error::Error::Other(format!(
            "encoded heads not a multiple of 32: {}",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 32);
    for chunk in bytes.chunks_exact(32) {
        let mut buf = [0u8; 32];
        buf.copy_from_slice(chunk);
        out.push(ChangeHash(buf));
    }
    Ok(out)
}
