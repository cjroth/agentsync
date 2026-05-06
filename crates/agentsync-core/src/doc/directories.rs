use crate::doc::{
    get_int, get_object, get_str, map_keys, new_id, now_ms, DirId, DirectoryMeta, Doc,
};
use crate::error::{Error, Result};
use crate::path;
use automerge::transaction::Transactable;
use automerge::ObjType;

impl Doc {
    pub fn find_directory_by_path(&mut self, path: &str) -> Result<Option<DirId>> {
        let path = path::normalize(path)?;
        self.find_directory_by_path_normalized(&path)
    }

    fn find_directory_by_path_normalized(&mut self, path: &str) -> Result<Option<DirId>> {
        let dirs = self.directories_obj()?;
        let keys = map_keys(&mut self.inner, &dirs);
        for did in keys {
            let meta = match get_object(&mut self.inner, &dirs, &did)? {
                Some(m) => m,
                None => continue,
            };
            if get_int(&mut self.inner, &meta, "deleted_at")?.is_some() {
                continue;
            }
            if let Some(p) = get_str(&mut self.inner, &meta, "path")? {
                if p == path {
                    return Ok(Some(did));
                }
            }
        }
        Ok(None)
    }

    pub fn create_directory(&mut self, path: &str) -> Result<DirId> {
        let path = path::normalize(path)?;
        if let Some(d) = self.find_directory_by_path_normalized(&path)? {
            return Ok(d);
        }
        let dirs = self.directories_obj()?;
        let did = new_id();
        let meta = self.inner.put_object(&dirs, &did, ObjType::Map)?;
        self.inner.put(&meta, "path", path.as_str())?;
        self.inner.put(&meta, "created_at", now_ms())?;
        self.ensure_ancestor_directories(&path)?;
        self.inner.commit();
        Ok(did)
    }

    pub(crate) fn ensure_ancestor_directories(&mut self, path: &str) -> Result<()> {
        let now = now_ms();
        for ancestor in path::ancestors(path) {
            if self.find_directory_by_path_normalized(&ancestor)?.is_some() {
                continue;
            }
            let dirs = self.directories_obj()?;
            let did = new_id();
            let meta = self.inner.put_object(&dirs, &did, ObjType::Map)?;
            self.inner.put(&meta, "path", ancestor.as_str())?;
            self.inner.put(&meta, "created_at", now)?;
        }
        Ok(())
    }

    pub fn delete_directory(&mut self, path: &str, recursive: bool) -> Result<()> {
        let path = path::normalize(path)?;
        let now = now_ms();

        // Identify children to cascade.
        let mut child_files = Vec::new();
        for f in self.list_files()? {
            if path::under(&path, &f.path) && f.path != path {
                child_files.push(f.path);
            }
        }
        let mut child_dirs: Vec<(String, String)> = Vec::new();
        let dirs = self.directories_obj()?;
        let dir_keys = map_keys(&mut self.inner, &dirs);
        for did in &dir_keys {
            let meta = match get_object(&mut self.inner, &dirs, did)? {
                Some(m) => m,
                None => continue,
            };
            if get_int(&mut self.inner, &meta, "deleted_at")?.is_some() {
                continue;
            }
            if let Some(p) = get_str(&mut self.inner, &meta, "path")? {
                if path::under(&path, &p) && p != path {
                    child_dirs.push((did.clone(), p));
                }
            }
        }
        if !child_files.is_empty() && !recursive {
            return Err(Error::Other(format!(
                "directory not empty: {} ({} child files)",
                path,
                child_files.len()
            )));
        }

        if recursive {
            for cf in child_files {
                self.delete_file(&cf)?;
            }
            let dirs = self.directories_obj()?;
            for (did, _) in child_dirs {
                if let Some(meta) = get_object(&mut self.inner, &dirs, &did)? {
                    self.inner.put(&meta, "deleted_at", now)?;
                }
            }
        }

        if let Some(did) = self.find_directory_by_path_normalized(&path)? {
            let dirs = self.directories_obj()?;
            if let Some(meta) = get_object(&mut self.inner, &dirs, &did)? {
                self.inner.put(&meta, "deleted_at", now)?;
            }
        }
        self.inner.commit();
        Ok(())
    }

    pub fn rename_directory(&mut self, from: &str, to: &str) -> Result<()> {
        let from = path::normalize(from)?;
        let to = path::normalize(to)?;
        if from == to {
            return Ok(());
        }
        let now = now_ms();

        if let Some(did) = self.find_directory_by_path_normalized(&from)? {
            let dirs = self.directories_obj()?;
            if let Some(meta) = get_object(&mut self.inner, &dirs, &did)? {
                self.inner.put(&meta, "path", to.as_str())?;
            }
        }

        // Children: directories
        let dirs_obj = self.directories_obj()?;
        let dir_keys = map_keys(&mut self.inner, &dirs_obj);
        for did in &dir_keys {
            let meta = match get_object(&mut self.inner, &dirs_obj, did)? {
                Some(m) => m,
                None => continue,
            };
            if get_int(&mut self.inner, &meta, "deleted_at")?.is_some() {
                continue;
            }
            let p = match get_str(&mut self.inner, &meta, "path")? {
                Some(p) => p,
                None => continue,
            };
            if path::under(&from, &p) && p != from {
                let new_p = format!("{}{}", to, &p[from.len()..]);
                self.inner.put(&meta, "path", new_p.as_str())?;
            }
        }
        // Children: files
        let files_obj = self.files_obj()?;
        let file_keys = map_keys(&mut self.inner, &files_obj);
        for fid in &file_keys {
            let entry = match get_object(&mut self.inner, &files_obj, fid)? {
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
            let p = match get_str(&mut self.inner, &meta, "path")? {
                Some(p) => p,
                None => continue,
            };
            if path::under(&from, &p) {
                let new_p = if p == from {
                    to.clone()
                } else {
                    format!("{}{}", to, &p[from.len()..])
                };
                self.inner.put(&meta, "path", new_p.as_str())?;
                self.inner.put(&meta, "updated_at", now)?;
            }
        }
        self.ensure_ancestor_directories(&to)?;
        self.inner.commit();
        Ok(())
    }

    pub fn list_directories(&mut self) -> Result<Vec<DirectoryMeta>> {
        let dirs = self.directories_obj()?;
        let mut out = Vec::new();
        let keys = map_keys(&mut self.inner, &dirs);
        for did in keys {
            let meta = match get_object(&mut self.inner, &dirs, &did)? {
                Some(m) => m,
                None => continue,
            };
            let path = match get_str(&mut self.inner, &meta, "path")? {
                Some(p) => p,
                None => continue,
            };
            let created_at = get_int(&mut self.inner, &meta, "created_at")?.unwrap_or(0);
            let deleted_at = get_int(&mut self.inner, &meta, "deleted_at")?;
            if deleted_at.is_some() {
                continue;
            }
            out.push(DirectoryMeta {
                id: did,
                path,
                created_at,
                deleted_at,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}
