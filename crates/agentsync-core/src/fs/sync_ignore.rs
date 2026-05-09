//! `.syncignore` support — gitignore-format files that exclude paths from
//! the sync engine. Discovered at bind time by walking the vault root for
//! every `.syncignore` file. Patterns are anchored to the directory of the
//! `.syncignore` they came from, matching git's nested-ignore semantics.
//!
//! Two exclusions are *not* user-tunable and live outside this set:
//! `.git/` and `.agentsync/`. They're hardcoded in `Binding::path_allowed`
//! because syncing them would corrupt the local repo or recursively sync the
//! vault state itself.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Filename we look for in every directory under the vault root.
pub const SYNC_IGNORE_FILENAME: &str = ".syncignore";

/// The set of `.syncignore` matchers discovered under a vault root. Each
/// entry is keyed by the directory the file lived in; the matcher is anchored
/// to that directory so patterns like `/foo` mean "foo at this level", just
/// like in git.
#[derive(Debug, Default, Clone)]
pub struct SyncIgnoreSet {
    /// Vault root the matchers are relative to. Used to resolve vault-relative
    /// paths back to absolute paths the underlying `Gitignore` expects.
    vault_root: PathBuf,
    matchers: Vec<DirMatcher>,
}

#[derive(Debug, Clone)]
struct DirMatcher {
    /// Absolute path to the directory containing the `.syncignore`.
    dir: PathBuf,
    /// Depth (count of components from the vault root). Used to pick the
    /// deepest applicable verdict — that's how git resolves overlapping
    /// nested ignore files.
    depth: usize,
    matcher: Gitignore,
}

impl SyncIgnoreSet {
    /// Walk `vault_root` for every `.syncignore` file and build a matcher per
    /// file. Skips directories that we never sync anyway (`.git/`,
    /// `.agentsync/`, `node_modules/`) so we don't blow time on giant trees.
    pub fn from_vault_root(vault_root: &Path) -> Self {
        let mut set = Self {
            vault_root: vault_root.to_path_buf(),
            matchers: Vec::new(),
        };
        if !vault_root.is_dir() {
            return set;
        }
        let walker = WalkDir::new(vault_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_skipped_dir(e.path(), vault_root));
        for entry in walker.flatten() {
            if entry.file_type().is_file() && entry.file_name() == SYNC_IGNORE_FILENAME {
                let dir = match entry.path().parent() {
                    Some(d) => d.to_path_buf(),
                    None => continue,
                };
                let mut builder = GitignoreBuilder::new(&dir);
                if builder.add(entry.path()).is_some() {
                    // `add` returns Some(error) on parse failure — skip this
                    // file rather than aborting the whole walk.
                    continue;
                }
                let Ok(matcher) = builder.build() else {
                    continue;
                };
                let depth = dir
                    .strip_prefix(vault_root)
                    .map(|p| p.components().count())
                    .unwrap_or(0);
                set.matchers.push(DirMatcher {
                    dir,
                    depth,
                    matcher,
                });
            }
        }
        // Shallowest first so deeper matchers override when we walk in order.
        set.matchers.sort_by_key(|m| m.depth);
        set
    }

    /// True if no `.syncignore` files were found. Lets the binding skip the
    /// per-path lookup when the user hasn't configured anything.
    pub fn is_empty(&self) -> bool {
        self.matchers.is_empty()
    }

    /// Resolve whether a vault-relative path is ignored. `is_dir` should be
    /// true for directory entries — gitignore's `foo/` form only matches dirs.
    /// Walks ancestors so files inside an ignored directory are also ignored.
    pub fn matches(&self, vault_relative: &str, is_dir: bool) -> bool {
        if self.matchers.is_empty() {
            return false;
        }
        let abs = self.vault_root.join(vault_relative);
        let mut ignored = false;
        // Apply matchers shallowest-first; deeper non-`None` verdicts override.
        for m in &self.matchers {
            if !abs.starts_with(&m.dir) {
                continue;
            }
            match m.matcher.matched_path_or_any_parents(&abs, is_dir) {
                ignore::Match::Ignore(_) => ignored = true,
                ignore::Match::Whitelist(_) => ignored = false,
                ignore::Match::None => {}
            }
        }
        ignored
    }
}

/// Skip descending into directories that are never synced regardless of
/// `.syncignore`. Avoids reading `.syncignore` files inside `node_modules` or
/// `.git`, which would be surprising and slow.
fn is_skipped_dir(path: &Path, vault_root: &Path) -> bool {
    if path == vault_root {
        return false;
    }
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".git" | ".agentsync" | "node_modules")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_when_no_files() {
        let dir = TempDir::new().unwrap();
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        assert!(set.is_empty());
        assert!(!set.matches("foo.md", false));
    }

    #[test]
    fn simple_pattern_excludes_file() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(".syncignore"), "secret.md\n");
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        assert!(set.matches("secret.md", false));
        assert!(!set.matches("public.md", false));
    }

    #[test]
    fn directory_pattern_excludes_children() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(".syncignore"), "build/\n");
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        assert!(set.matches("build", true));
        assert!(set.matches("build/output.txt", false));
    }

    #[test]
    fn negation_overrides_earlier_pattern() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(".syncignore"), "*.log\n!keep.log\n");
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        assert!(set.matches("foo.log", false));
        assert!(!set.matches("keep.log", false));
    }

    #[test]
    fn nested_syncignore_anchors_to_its_directory() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(".syncignore"), "*.tmp\n");
        write(&dir.path().join("sub/.syncignore"), "private/\n");
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        // Top-level rule applies anywhere
        assert!(set.matches("a.tmp", false));
        assert!(set.matches("sub/a.tmp", false));
        // Nested rule applies only under sub/
        assert!(set.matches("sub/private/x", false));
        assert!(!set.matches("private/x", false));
    }

    #[test]
    fn skips_well_known_dirs() {
        let dir = TempDir::new().unwrap();
        // A `.syncignore` inside `.git/` shouldn't be loaded.
        write(&dir.path().join(".git/.syncignore"), "*\n");
        let set = SyncIgnoreSet::from_vault_root(dir.path());
        assert!(set.is_empty());
    }
}
