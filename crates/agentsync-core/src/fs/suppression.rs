use std::collections::HashMap;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(5);

/// Records (path, content_hash) pairs that this peer wrote in response to an
/// incoming sync change. When the filesystem watcher subsequently fires for
/// the same path with the same content, we ignore it to avoid an echo loop.
#[derive(Default)]
pub struct DirtySet {
    entries: HashMap<String, Vec<(String, Instant)>>,
}

impl DirtySet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `path` as having been written by core with `content_hash`.
    pub fn mark(&mut self, path: &str, content_hash: &str) {
        self.gc();
        self.entries
            .entry(path.to_string())
            .or_default()
            .push((content_hash.to_string(), Instant::now()));
    }

    /// Returns true if a watcher event for `path` with `content_hash` should be ignored.
    /// Consumes the matching entry on success (one-shot suppression).
    pub fn check_and_consume(&mut self, path: &str, content_hash: &str) -> bool {
        self.gc();
        let entries = match self.entries.get_mut(path) {
            Some(v) => v,
            None => return false,
        };
        if let Some(idx) = entries.iter().position(|(h, _)| h == content_hash) {
            entries.remove(idx);
            if entries.is_empty() {
                self.entries.remove(path);
            }
            return true;
        }
        false
    }

    fn gc(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, v| {
            v.retain(|(_, t)| now.duration_since(*t) < TTL);
            !v.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_consume() {
        let mut s = DirtySet::new();
        s.mark("a.md", "h1");
        assert!(s.check_and_consume("a.md", "h1"));
        assert!(!s.check_and_consume("a.md", "h1"));
    }

    #[test]
    fn different_content_not_suppressed() {
        let mut s = DirtySet::new();
        s.mark("a.md", "h1");
        assert!(!s.check_and_consume("a.md", "h2"));
    }
}
