use crate::doc::{
    get_int, get_object, get_text, map_keys, now_ms, Doc, FileKind, Label,
};
use crate::error::{Error, Result};
use automerge::transaction::Transactable;
use automerge::{ChangeHash, ObjType, ReadDoc, ScalarValue, Value};
use base64::Engine;

impl Doc {
    pub fn create_label(&mut self, label: &str) -> Result<()> {
        let labels = self.labels_obj()?;
        let heads = self.inner.get_heads();
        let encoded = encode_heads(&heads);
        let entry = self.inner.put_object(&labels, label, ObjType::Map)?;
        self.inner.put(&entry, "heads", ScalarValue::Bytes(encoded))?;
        self.inner.put(&entry, "created_at", now_ms())?;
        self.commit_now();
        Ok(())
    }

    pub fn delete_label(&mut self, label: &str) -> Result<()> {
        let labels = self.labels_obj()?;
        self.inner.delete(&labels, label)?;
        self.commit_now();
        Ok(())
    }

    pub fn list_labels(&mut self) -> Result<Vec<Label>> {
        let labels = self.labels_obj()?;
        let names = map_keys(&mut self.inner, &labels);
        let mut out = Vec::new();
        for name in names {
            let value = self.inner.get(&labels, &name)?;
            match value {
                Some((Value::Object(_), id)) => {
                    let bytes = match self.inner.get(&id, "heads")? {
                        Some((Value::Scalar(s), _)) => match s.as_ref() {
                            ScalarValue::Bytes(b) => b.clone(),
                            _ => continue,
                        },
                        _ => continue,
                    };
                    let created_at = get_int(&mut self.inner, &id, "created_at")?.unwrap_or(0);
                    let heads = decode_heads(&bytes)?;
                    out.push(Label {
                        name,
                        heads,
                        created_at,
                    });
                }
                Some((Value::Scalar(s), _)) => {
                    if let ScalarValue::Bytes(bytes) = s.as_ref() {
                        let heads = decode_heads(bytes)?;
                        out.push(Label {
                            name,
                            heads,
                            created_at: 0,
                        });
                    }
                }
                _ => continue,
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn get_label_heads(&mut self, label: &str) -> Result<Vec<ChangeHash>> {
        for l in self.list_labels()? {
            if l.name == label {
                return Ok(l.heads);
            }
        }
        Err(Error::NotFound(format!("label: {}", label)))
    }

    /// Restore the vault state to match `heads`. Implemented additively:
    /// produce new forward-going changes that bring the document state to match
    /// the past snapshot, while preserving any concurrent changes from other peers.
    pub fn restore_to_heads(&mut self, heads: &[ChangeHash]) -> Result<()> {
        let mut past = Doc {
            inner: self
                .inner
                .fork_at(heads)
                .map_err(|e| Error::Other(format!("fork_at failed: {}", e)))?,
        };

        // Snapshot past state.
        let past_files: std::collections::HashMap<String, _> = past
            .list_files()?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let past_dirs: std::collections::HashMap<String, _> = past
            .list_directories()?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let mut past_file_contents: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let past_ids: Vec<String> = past_files
            .iter()
            .filter(|(_, m)| matches!(m.kind, FileKind::Text))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &past_ids {
            if let Ok(Some(c)) = past.read_file_id(id) {
                past_file_contents.insert(id.clone(), c);
            }
        }

        let now = now_ms();
        let cur_files = self.files_obj()?;
        let cur_dirs = self.directories_obj()?;
        let cur_file_ids: Vec<String> = map_keys(&mut self.inner, &cur_files);
        let cur_dir_ids: Vec<String> = map_keys(&mut self.inner, &cur_dirs);

        for fid in &cur_file_ids {
            let alive_now = self
                .read_file_meta(fid)?
                .map(|m| m.deleted_at.is_none())
                .unwrap_or(false);
            let alive_past = past_files
                .get(fid)
                .map(|m| m.deleted_at.is_none())
                .unwrap_or(false);
            if alive_now && !alive_past {
                let cur_files = self.files_obj()?;
                if let Some(entry) = get_object(&mut self.inner, &cur_files, fid)? {
                    if let Some(meta) = get_object(&mut self.inner, &entry, "meta")? {
                        self.inner.put(&meta, "deleted_at", now)?;
                    }
                }
            }
        }

        for (fid, past_meta) in &past_files {
            if past_meta.deleted_at.is_some() {
                continue;
            }
            let cur_files = self.files_obj()?;
            let entry = match get_object(&mut self.inner, &cur_files, fid)? {
                Some(e) => e,
                None => self.inner.put_object(&cur_files, fid, ObjType::Map)?,
            };
            let meta = match get_object(&mut self.inner, &entry, "meta")? {
                Some(m) => m,
                None => self.inner.put_object(&entry, "meta", ObjType::Map)?,
            };
            self.inner.put(&meta, "path", past_meta.path.as_str())?;
            self.inner.put(&meta, "kind", past_meta.kind.as_str())?;
            self.inner.put(&meta, "size", past_meta.size)?;
            self.inner.put(&meta, "created_at", past_meta.created_at)?;
            self.inner.put(&meta, "updated_at", now)?;
            self.inner.delete(&meta, "deleted_at")?;
            match past_meta.kind {
                FileKind::Text => {
                    let target = past_file_contents
                        .get(fid)
                        .cloned()
                        .unwrap_or_default();
                    let cur_text = get_text(&mut self.inner, &entry, "content")?;
                    match cur_text {
                        Some((id, current)) => {
                            if current != target {
                                let len = current.chars().count();
                                self.inner.splice_text(&id, 0, len as isize, &target)?;
                            }
                        }
                        None => {
                            let id = self
                                .inner
                                .put_object(&entry, "content", ObjType::Text)?;
                            if !target.is_empty() {
                                self.inner.splice_text(&id, 0, 0, &target)?;
                            }
                        }
                    }
                }
                FileKind::Attachment => {
                    if let Some(h) = &past_meta.binary_hash {
                        self.inner.put(&entry, "binary_hash", h.as_str())?;
                    }
                }
            }
        }

        for did in &cur_dir_ids {
            let cur_dirs = self.directories_obj()?;
            let alive_now = match get_object(&mut self.inner, &cur_dirs, did)? {
                Some(meta) => get_int(&mut self.inner, &meta, "deleted_at")?.is_none(),
                None => false,
            };
            let alive_past = past_dirs
                .get(did)
                .map(|m| m.deleted_at.is_none())
                .unwrap_or(false);
            if alive_now && !alive_past {
                let cur_dirs = self.directories_obj()?;
                if let Some(meta) = get_object(&mut self.inner, &cur_dirs, did)? {
                    self.inner.put(&meta, "deleted_at", now)?;
                }
            }
        }
        for (did, past_meta) in &past_dirs {
            if past_meta.deleted_at.is_some() {
                continue;
            }
            let cur_dirs = self.directories_obj()?;
            let meta = match get_object(&mut self.inner, &cur_dirs, did)? {
                Some(m) => m,
                None => self.inner.put_object(&cur_dirs, did, ObjType::Map)?,
            };
            self.inner.put(&meta, "path", past_meta.path.as_str())?;
            self.inner.put(&meta, "created_at", past_meta.created_at)?;
            self.inner.delete(&meta, "deleted_at")?;
        }

        self.commit_now();
        Ok(())
    }

    pub fn restore_to_time(&mut self, target_ms: i64) -> Result<()> {
        let target_heads = self.heads_at_time(target_ms)?;
        self.restore_to_heads(&target_heads)
    }

    pub fn heads_at_time(&mut self, target_ms: i64) -> Result<Vec<ChangeHash>> {
        let changes = self.inner.get_changes(&[]);
        let mut included: std::collections::HashSet<ChangeHash> =
            std::collections::HashSet::new();
        for c in &changes {
            if c.timestamp() <= target_ms {
                included.insert(c.hash());
            }
        }
        let mut heads: std::collections::HashSet<ChangeHash> = included.clone();
        for c in &changes {
            if !included.contains(&c.hash()) {
                continue;
            }
            for dep in c.deps() {
                heads.remove(dep);
            }
        }
        Ok(heads.into_iter().collect())
    }

    /// Diagnostic: list every change in the document with its timestamp and
    /// dep hashes. Used by tests/repros that need to reason about history shape.
    #[doc(hidden)]
    pub fn debug_changes(&mut self) -> Vec<(ChangeHash, i64, Vec<ChangeHash>)> {
        self.inner
            .get_changes(&[])
            .into_iter()
            .map(|c| (c.hash(), c.timestamp(), c.deps().to_vec()))
            .collect()
    }

    pub fn read_file_id(&mut self, fid: &str) -> Result<Option<String>> {
        let files = self.files_obj()?;
        let entry = match get_object(&mut self.inner, &files, fid)? {
            Some(e) => e,
            None => return Ok(None),
        };
        if let Some((_, text)) = get_text(&mut self.inner, &entry, "content")? {
            return Ok(Some(text));
        }
        Ok(None)
    }
}

fn encode_heads(heads: &[ChangeHash]) -> Vec<u8> {
    let mut out = Vec::with_capacity(heads.len() * 32);
    for h in heads {
        out.extend_from_slice(h.as_ref());
    }
    out
}

fn decode_heads(bytes: &[u8]) -> Result<Vec<ChangeHash>> {
    if bytes.len() % 32 != 0 {
        return Err(Error::Other(format!(
            "encoded heads length not multiple of 32: {}",
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

#[allow(dead_code)]
pub fn b64_heads(heads: &[ChangeHash]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(encode_heads(heads))
}
