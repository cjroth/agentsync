use crate::error::{Error, Result};
use unicode_normalization::UnicodeNormalization;

/// Normalize a path to POSIX/NFC form. Used at the core boundary.
pub fn normalize(input: &str) -> Result<String> {
    if input.is_empty() {
        return Err(Error::InvalidPath("path is empty".into()));
    }

    let unified: String = input.chars().map(|c| if c == '\\' { '/' } else { c }).collect();
    let nfc: String = unified.nfc().collect();

    // Reject absolute paths and parent traversal.
    if nfc.starts_with('/') {
        return Err(Error::InvalidPath(format!(
            "path must be relative: {}",
            nfc
        )));
    }
    let mut parts = Vec::new();
    for seg in nfc.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(Error::InvalidPath(format!(
                "path traversal not allowed: {}",
                nfc
            )));
        }
        if seg.contains('\0') {
            return Err(Error::InvalidPath("nul byte in path".into()));
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return Err(Error::InvalidPath("path resolves to empty".into()));
    }
    Ok(parts.join("/"))
}

/// Returns the parent directory portion of `path`, or empty string for root-level.
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// All ancestor directories of a normalized path, from root toward leaf,
/// excluding the path itself.
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 1 {
        return out;
    }
    for seg in &parts[..parts.len() - 1] {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(acc.clone());
    }
    out
}

/// Returns true if `child` is `prefix` or a descendant of `prefix`.
pub fn under(prefix: &str, child: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if child == prefix {
        return true;
    }
    child.starts_with(prefix) && child.as_bytes().get(prefix.len()) == Some(&b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_normalization() {
        assert_eq!(normalize("a/b/c.md").unwrap(), "a/b/c.md");
        assert_eq!(normalize("a\\b\\c.md").unwrap(), "a/b/c.md");
        assert_eq!(normalize("./a/./b").unwrap(), "a/b");
        assert_eq!(normalize("a//b").unwrap(), "a/b");
    }

    #[test]
    fn rejects_traversal() {
        assert!(normalize("../a").is_err());
        assert!(normalize("a/../b").is_err());
        assert!(normalize("/a").is_err());
        assert!(normalize("").is_err());
    }

    #[test]
    fn ancestors_of() {
        assert_eq!(ancestors("a/b/c"), vec!["a".to_string(), "a/b".to_string()]);
        assert_eq!(ancestors("a"), Vec::<String>::new());
    }

    #[test]
    fn under_works() {
        assert!(under("a", "a/b"));
        assert!(under("a", "a"));
        assert!(!under("a", "ab"));
        assert!(under("", "anything"));
    }
}
