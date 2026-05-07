use crate::doc::{
    content_hash, get_int, get_object, get_str, get_text, map_keys, new_id, now_ms, Doc, FileId,
    FileKind, FileMeta,
};
use crate::error::{Error, Result};
use crate::path;
use automerge::transaction::Transactable;
use automerge::ObjType;

impl Doc {
    pub fn find_file_by_path(&mut self, path: &str) -> Result<Option<FileId>> {
        let path = path::normalize(path)?;
        self.find_file_by_path_normalized(&path)
    }

    fn find_file_by_path_normalized(&mut self, path: &str) -> Result<Option<FileId>> {
        let files = self.files_obj()?;
        let keys = map_keys(&mut self.inner, &files);
        for fid in keys {
            let entry = match get_object(&mut self.inner, &files, &fid)? {
                Some(e) => e,
                None => continue,
            };
            let meta = match get_object(&mut self.inner, &entry, "meta")? {
                Some(m) => m,
                None => continue,
            };
            if get_int(&mut self.inner, &meta, "deleted_at")?.is_some() {
                continue;
            }
            if let Some(p) = get_str(&mut self.inner, &meta, "path")? {
                if p == path {
                    return Ok(Some(fid));
                }
            }
        }
        Ok(None)
    }

    pub fn file_exists(&mut self, path: &str) -> bool {
        self.find_file_by_path(path).ok().flatten().is_some()
    }

    pub fn read_file(&mut self, path: &str) -> Result<String> {
        let path = path::normalize(path)?;
        let fid = self
            .find_file_by_path_normalized(&path)?
            .ok_or_else(|| Error::NotFound(path.clone()))?;
        let files = self.files_obj()?;
        let entry = get_object(&mut self.inner, &files, &fid)?
            .ok_or_else(|| Error::Other(format!("file entry vanished: {}", fid)))?;
        if let Some((_, text)) = get_text(&mut self.inner, &entry, "content")? {
            return Ok(text);
        }
        Err(Error::Other(format!("file is not a text file: {}", path)))
    }

    pub fn file_hash(&mut self, path: &str) -> Result<String> {
        let path = path::normalize(path)?;
        let fid = self
            .find_file_by_path_normalized(&path)?
            .ok_or_else(|| Error::NotFound(path.clone()))?;
        let files = self.files_obj()?;
        let entry = get_object(&mut self.inner, &files, &fid)?
            .ok_or_else(|| Error::Other(format!("file entry vanished: {}", fid)))?;
        let meta = get_object(&mut self.inner, &entry, "meta")?
            .ok_or_else(|| Error::Other("file meta missing".into()))?;
        if let Some(kind_s) = get_str(&mut self.inner, &meta, "kind")? {
            if FileKind::parse(&kind_s)? == FileKind::Attachment {
                if let Some(h) = get_str(&mut self.inner, &entry, "binary_hash")? {
                    return Ok(h);
                }
            }
        }
        if let Some((_, text)) = get_text(&mut self.inner, &entry, "content")? {
            return Ok(content_hash(text.as_bytes()));
        }
        Err(Error::Other(format!("cannot hash file: {}", path)))
    }

    pub fn list_files(&mut self) -> Result<Vec<FileMeta>> {
        let files = self.files_obj()?;
        let keys = map_keys(&mut self.inner, &files);
        let mut out = Vec::new();
        for fid in keys {
            if let Some(meta) = self.read_file_meta(&fid)? {
                if meta.deleted_at.is_none() {
                    out.push(meta);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn list_file_paths(&mut self) -> Result<Vec<String>> {
        Ok(self.list_files()?.into_iter().map(|m| m.path).collect())
    }

    pub fn read_file_meta(&mut self, fid: &str) -> Result<Option<FileMeta>> {
        let files = self.files_obj()?;
        let entry = match get_object(&mut self.inner, &files, fid)? {
            Some(e) => e,
            None => return Ok(None),
        };
        let meta = match get_object(&mut self.inner, &entry, "meta")? {
            Some(m) => m,
            None => return Ok(None),
        };
        let path = get_str(&mut self.inner, &meta, "path")?
            .ok_or_else(|| Error::Other(format!("file {} missing path", fid)))?;
        let kind_s = get_str(&mut self.inner, &meta, "kind")?
            .ok_or_else(|| Error::Other(format!("file {} missing kind", fid)))?;
        let kind = FileKind::parse(&kind_s)?;
        let size = get_int(&mut self.inner, &meta, "size")?.unwrap_or(0);
        let created_at = get_int(&mut self.inner, &meta, "created_at")?.unwrap_or(0);
        let updated_at = get_int(&mut self.inner, &meta, "updated_at")?.unwrap_or(created_at);
        let deleted_at = get_int(&mut self.inner, &meta, "deleted_at")?;
        let binary_hash = get_str(&mut self.inner, &entry, "binary_hash")?;
        Ok(Some(FileMeta {
            id: fid.to_string(),
            path,
            kind,
            size,
            created_at,
            updated_at,
            deleted_at,
            binary_hash,
        }))
    }

    pub fn write_text_file(&mut self, path: &str, content: &str) -> Result<FileId> {
        let path = path::normalize(path)?;
        let now = now_ms();
        let size = content.len() as i64;

        if let Some(fid) = self.find_file_by_path_normalized(&path)? {
            let files = self.files_obj()?;
            let entry = get_object(&mut self.inner, &files, &fid)?.unwrap();
            let meta = get_object(&mut self.inner, &entry, "meta")?.unwrap();
            if let Some((text_id, current)) = get_text(&mut self.inner, &entry, "content")? {
                if current != content {
                    let len = current.chars().count();
                    self.inner
                        .splice_text(&text_id, 0, len as isize, content)?;
                }
            } else {
                let text_id = self.inner.put_object(&entry, "content", ObjType::Text)?;
                if !content.is_empty() {
                    self.inner.splice_text(&text_id, 0, 0, content)?;
                }
            }
            self.inner.put(&meta, "size", size)?;
            self.inner.put(&meta, "updated_at", now)?;
            self.inner.put(&meta, "kind", FileKind::Text.as_str())?;
            self.inner.delete(&meta, "deleted_at")?;
            self.commit_now();
            return Ok(fid);
        }

        let files = self.files_obj()?;
        let fid = new_id();
        let entry = self.inner.put_object(&files, &fid, ObjType::Map)?;
        let meta = self.inner.put_object(&entry, "meta", ObjType::Map)?;
        self.inner.put(&meta, "path", path.as_str())?;
        self.inner.put(&meta, "kind", FileKind::Text.as_str())?;
        self.inner.put(&meta, "size", size)?;
        self.inner.put(&meta, "created_at", now)?;
        self.inner.put(&meta, "updated_at", now)?;
        let text_id = self.inner.put_object(&entry, "content", ObjType::Text)?;
        if !content.is_empty() {
            self.inner.splice_text(&text_id, 0, 0, content)?;
        }
        self.ensure_ancestor_directories(&path)?;
        self.commit_now();
        Ok(fid)
    }

    pub fn write_attachment(&mut self, path: &str, hash: &str, size: i64) -> Result<FileId> {
        let path = path::normalize(path)?;
        let now = now_ms();

        if let Some(fid) = self.find_file_by_path_normalized(&path)? {
            let files = self.files_obj()?;
            let entry = get_object(&mut self.inner, &files, &fid)?.unwrap();
            let meta = get_object(&mut self.inner, &entry, "meta")?.unwrap();
            self.inner.put(&entry, "binary_hash", hash)?;
            self.inner.put(&meta, "kind", FileKind::Attachment.as_str())?;
            self.inner.put(&meta, "size", size)?;
            self.inner.put(&meta, "updated_at", now)?;
            self.inner.delete(&meta, "deleted_at")?;
            self.commit_now();
            return Ok(fid);
        }

        let files = self.files_obj()?;
        let fid = new_id();
        let entry = self.inner.put_object(&files, &fid, ObjType::Map)?;
        let meta = self.inner.put_object(&entry, "meta", ObjType::Map)?;
        self.inner.put(&meta, "path", path.as_str())?;
        self.inner.put(&meta, "kind", FileKind::Attachment.as_str())?;
        self.inner.put(&meta, "size", size)?;
        self.inner.put(&meta, "created_at", now)?;
        self.inner.put(&meta, "updated_at", now)?;
        self.inner.put(&entry, "binary_hash", hash)?;
        self.ensure_ancestor_directories(&path)?;
        self.commit_now();
        Ok(fid)
    }

    pub fn delete_file(&mut self, path: &str) -> Result<()> {
        let path = path::normalize(path)?;
        let fid = self
            .find_file_by_path_normalized(&path)?
            .ok_or_else(|| Error::NotFound(path.clone()))?;
        let files = self.files_obj()?;
        let entry = get_object(&mut self.inner, &files, &fid)?.unwrap();
        let meta = get_object(&mut self.inner, &entry, "meta")?.unwrap();
        self.inner.put(&meta, "deleted_at", now_ms())?;
        self.commit_now();
        Ok(())
    }

    pub fn rename_file(&mut self, from: &str, to: &str) -> Result<()> {
        let from = path::normalize(from)?;
        let to = path::normalize(to)?;
        if from == to {
            return Ok(());
        }
        let fid = self
            .find_file_by_path_normalized(&from)?
            .ok_or_else(|| Error::NotFound(from.clone()))?;
        if self.find_file_by_path_normalized(&to)?.is_some() {
            return Err(Error::AlreadyExists(to));
        }
        let files = self.files_obj()?;
        let entry = get_object(&mut self.inner, &files, &fid)?.unwrap();
        let meta = get_object(&mut self.inner, &entry, "meta")?.unwrap();
        self.inner.put(&meta, "path", to.as_str())?;
        self.inner.put(&meta, "updated_at", now_ms())?;
        self.ensure_ancestor_directories(&to)?;
        self.commit_now();
        Ok(())
    }
}
