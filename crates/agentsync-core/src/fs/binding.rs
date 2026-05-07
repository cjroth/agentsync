use crate::constants::AUTHORIZED_KEYS_FILE;
use crate::fs::adapter::{FilesystemAdapter, Watcher};
use crate::fs::suppression::DirtySet;
use crate::path as path_norm;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration for binding the vault to a local directory.
#[derive(Debug, Clone)]
pub struct BindOptions {
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
    /// Extensions (without the dot) that should be ingested as Automerge text.
    /// A file whose extension is not in this list and is not matched by
    /// `include_patterns` is ignored entirely; a file whose extension is not
    /// in this list but IS in `include_patterns` is ingested as a binary
    /// attachment.
    pub text_extensions: Vec<String>,
    pub attachment_max_bytes: u64,
    pub text_file_max_bytes: u64,
}

impl BindOptions {
    /// Markdown-only defaults. To allow more file types, populate
    /// `text_extensions` and `include_patterns` (or build via
    /// `BindOptions::for_extensions`).
    pub fn markdown_only() -> Self {
        Self::for_extensions(["md", "markdown"])
    }

    /// Build options that allow the given list of extensions. The include
    /// filter is set to `**/*.<ext>` for each, and the text-extension list is
    /// the same set so all listed extensions are stored as Automerge text.
    pub fn for_extensions<I, S>(exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let exts: Vec<String> = exts
            .into_iter()
            .map(|s| s.as_ref().trim_start_matches('.').to_ascii_lowercase())
            .collect();
        let include = exts.iter().map(|e| format!("**/*.{}", e)).collect();
        Self {
            exclude_patterns: default_exclude_patterns(),
            include_patterns: include,
            text_extensions: exts,
            attachment_max_bytes: 10 * 1024 * 1024,
            text_file_max_bytes: 1 * 1024 * 1024,
        }
    }
}

impl Default for BindOptions {
    fn default() -> Self {
        Self::markdown_only()
    }
}

fn default_exclude_patterns() -> Vec<String> {
    vec![
        "**/.git/**".into(),
        "**/node_modules/**".into(),
        "**/.DS_Store".into(),
        "**/.agentsync/**".into(),
    ]
}

/// Per-binding state shared between the inbound fs loop and the outbound
/// materializer. The Vault holds an Arc<Binding> while a directory is bound.
pub struct Binding {
    root: PathBuf,
    opts: BindOptions,
    adapter: Arc<dyn FilesystemAdapter>,
    pub(crate) dirty: Arc<Mutex<DirtySet>>,
    /// path-in-doc -> content hash currently materialized on disk
    pub(crate) materialized: Arc<Mutex<HashMap<String, String>>>,
    /// path-in-doc -> hash of the disk content that we most recently ingested
    /// into the doc. Used by the materializer to recognise that a disk-state
    /// the user just saved has already been captured by the doc, so it's safe
    /// to overwrite with the doc's (possibly merged) content.
    pub(crate) last_ingested: Arc<Mutex<HashMap<String, String>>>,
    /// Set of directory paths the materializer has created on disk (or
    /// confirmed already exist after the initial scan). Used to detect when a
    /// directory has been deleted in the doc and should be removed locally.
    pub(crate) materialized_dirs: Arc<Mutex<HashSet<String>>>,
    _watcher: Option<Box<dyn Watcher>>,
}

impl Binding {
    pub fn new(
        root: impl AsRef<Path>,
        opts: BindOptions,
        adapter: Arc<dyn FilesystemAdapter>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            opts,
            adapter,
            dirty: Arc::new(Mutex::new(DirtySet::new())),
            materialized: Arc::new(Mutex::new(HashMap::new())),
            last_ingested: Arc::new(Mutex::new(HashMap::new())),
            materialized_dirs: Arc::new(Mutex::new(HashSet::new())),
            _watcher: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn opts(&self) -> &BindOptions {
        &self.opts
    }

    pub fn adapter(&self) -> &Arc<dyn FilesystemAdapter> {
        &self.adapter
    }

    pub fn set_watcher(&mut self, w: Box<dyn Watcher>) {
        self._watcher = Some(w);
    }

    /// Translate an absolute filesystem path under `root` to a logical
    /// vault-relative POSIX path. Returns None if the path is outside the root
    /// or excluded.
    pub fn fs_path_to_vault_path(&self, abs: &Path) -> Option<String> {
        let rel = abs.strip_prefix(&self.root).ok()?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            return None;
        }
        let normalized = path_norm::normalize(&rel_str).ok()?;
        if !self.path_allowed(&normalized) {
            return None;
        }
        Some(normalized)
    }

    /// Like `fs_path_to_vault_path` but applies only the exclude rules.
    /// Directories are not subject to the file-extension include filter, so
    /// e.g. an empty `notes/` folder still syncs even when only `*.md` files
    /// are included.
    pub fn fs_path_to_vault_dir_path(&self, abs: &Path) -> Option<String> {
        let rel = abs.strip_prefix(&self.root).ok()?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            return None;
        }
        let normalized = path_norm::normalize(&rel_str).ok()?;
        if !self.dir_path_allowed(&normalized) {
            return None;
        }
        Some(normalized)
    }

    pub fn vault_path_to_fs_path(&self, vault: &str) -> PathBuf {
        let mut p = self.root.clone();
        for seg in vault.split('/') {
            p.push(seg);
        }
        p
    }

    fn path_allowed(&self, path: &str) -> bool {
        if !self.opts.exclude_patterns.is_empty()
            && glob_match_any(&self.opts.exclude_patterns, path)
        {
            return false;
        }
        // The hub gates connections on this file, so it must sync even when
        // the user's include filter (markdown-only by default) would skip it.
        if path == AUTHORIZED_KEYS_FILE {
            return true;
        }
        if !self.opts.include_patterns.is_empty()
            && !glob_match_any(&self.opts.include_patterns, path)
        {
            return false;
        }
        true
    }

    /// Whether a directory path is allowed. Only the exclude list applies —
    /// the include list is meant for filtering files by extension and would
    /// reject every directory if applied here.
    pub(crate) fn dir_path_allowed(&self, path: &str) -> bool {
        if self.opts.exclude_patterns.is_empty() {
            return true;
        }
        !glob_match_any(&self.opts.exclude_patterns, path)
    }

    pub(crate) fn is_text_extension(&self, path: &str) -> bool {
        // Pair with the special-case in `path_allowed`: the auth file has no
        // extension but its content is text and must round-trip exactly.
        if path == AUTHORIZED_KEYS_FILE {
            return true;
        }
        match Path::new(path).extension().and_then(|s| s.to_str()) {
            Some(ext) => self
                .opts
                .text_extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext)),
            // Dotfiles and extensionless files are treated as text iff the
            // text-extensions list is empty (legacy behavior) or contains "".
            None => self.opts.text_extensions.is_empty(),
        }
    }

    pub(crate) fn over_size(&self, path: &str, size: u64) -> bool {
        if self.is_text_extension(path) {
            size > self.opts.text_file_max_bytes
        } else {
            size > self.opts.attachment_max_bytes
        }
    }
}

/// Very small glob matcher supporting `**` and `*` only. Sufficient for the
/// default patterns; if users need full gitignore semantics we can swap in
/// the `globset` crate.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn matches(pat: &[u8], s: &[u8]) -> bool {
        let mut pi = 0;
        let mut si = 0;
        let mut star_pi: Option<usize> = None;
        let mut star_si = 0;
        while si < s.len() {
            if pi < pat.len() {
                if pi + 1 < pat.len() && pat[pi] == b'*' && pat[pi + 1] == b'*' {
                    // double-star matches across slashes
                    star_pi = Some(pi);
                    pi += 2;
                    if pi < pat.len() && pat[pi] == b'/' {
                        pi += 1;
                    }
                    star_si = si;
                    continue;
                }
                if pat[pi] == b'*' {
                    star_pi = Some(pi);
                    pi += 1;
                    star_si = si;
                    continue;
                }
                if pat[pi] == s[si] {
                    pi += 1;
                    si += 1;
                    continue;
                }
            }
            if let Some(p) = star_pi {
                pi = p + 1;
                if pat.get(p) == Some(&b'*') && pat.get(p + 1) == Some(&b'*') {
                    pi = p + 2;
                    if pat.get(pi) == Some(&b'/') {
                        pi += 1;
                    }
                }
                star_si += 1;
                si = star_si;
                continue;
            }
            return false;
        }
        // Trailing star(s)
        while pi < pat.len() && (pat[pi] == b'*') {
            pi += 1;
        }
        pi == pat.len()
    }
    matches(pattern.as_bytes(), path.as_bytes())
}

pub fn glob_match_any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.md", "a.md"));
        assert!(!glob_match("*.md", "a.txt"));
        assert!(glob_match("**/.git/**", "x/y/.git/HEAD"));
        assert!(glob_match("**/*.md", "x/y/z.md"));
        assert!(glob_match("**/.agentsync/**", ".agentsync/doc.bin"));
    }
}
