//! Parser for `authorized_keys` — the SSH-style file at the root of a vault
//! that lists authorized device pubkeys.
//!
//! Format mirrors `~/.ssh/authorized_keys`: each line is
//! `ssh-ed25519 <base64> [comment]`. Lines beginning with `#` are comments.
//! Blank lines are ignored. Users can paste OpenSSH pubkey lines directly.
//!
//! The legacy markdown bullet form (`- `ssh-ed25519 ...` — alice`) is also
//! accepted so half-migrated vaults don't break, but `render_authorized_keys`
//! always emits the SSH-style form.

use crate::constants::AUTHORIZED_KEYS_FILE;
use crate::identity::Pubkey;

/// Backwards-compat alias. Prefer [`AUTHORIZED_KEYS_FILE`] in new code.
pub const PEERS_FILE: &str = AUTHORIZED_KEYS_FILE;

#[derive(Debug, Clone)]
pub struct AuthorizedPeer {
    pub pubkey: Pubkey,
    pub label: String,
}

/// Parse the SSH-style file content into a list of authorized peers.
/// Unparseable / commented / blank lines are silently skipped.
pub fn parse_authorized_keys(content: &str) -> Vec<AuthorizedPeer> {
    let mut out = Vec::new();
    for raw in content.lines() {
        if let Some(peer) = parse_line(raw) {
            out.push(peer);
        }
    }
    out
}

/// Backwards-compat alias.
pub fn parse_peers_md(content: &str) -> Vec<AuthorizedPeer> {
    parse_authorized_keys(content)
}

fn parse_line(raw: &str) -> Option<AuthorizedPeer> {
    let trimmed = raw.trim_start_matches(|c: char| c.is_whitespace());
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') {
        return None;
    }
    // Accept the legacy markdown bullet form too. `- ` or `* ` prefix optional.
    let body = strip_list_marker(trimmed).unwrap_or(trimmed);

    // Strip wrapping backticks from the key portion (legacy form).
    if body.starts_with('`') {
        return parse_legacy(body);
    }

    let mut parts = body.splitn(3, char::is_whitespace);
    let kind = parts.next()?;
    let blob = parts.next()?;
    let label = parts.next().map(|s| s.trim()).unwrap_or("").to_string();
    let key_text = format!("{} {}", kind, blob);
    let pk = Pubkey::from_ssh_string(&key_text).ok()?;
    Some(AuthorizedPeer { pubkey: pk, label })
}

/// Parse one of the old `- `ssh-ed25519 ...` — alice` markdown bullet
/// lines. Only reachable when a line opens with a backtick. Kept so that
/// users who have an old rendered `peers.md` lying around can paste it
/// straight into the new `authorized_keys` without hand-editing.
fn parse_legacy(body: &str) -> Option<AuthorizedPeer> {
    let (key_part, label) = split_key_and_label(body);
    let key_text = key_part.trim().trim_matches('`').trim();
    let pk = Pubkey::from_ssh_string(key_text).ok()?;
    Some(AuthorizedPeer {
        pubkey: pk,
        label: label.to_string(),
    })
}

fn strip_list_marker(line: &str) -> Option<&str> {
    for marker in ["- ", "* "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

fn split_key_and_label(s: &str) -> (&str, &str) {
    for sep in [" — ", " – ", " -- ", " - "] {
        if let Some(idx) = s.find(sep) {
            let (k, rest) = s.split_at(idx);
            return (k, rest[sep.len()..].trim());
        }
    }
    (s, "")
}

/// Render a list of authorized peers in SSH `authorized_keys` format.
pub fn render_authorized_keys(peers: &[AuthorizedPeer]) -> String {
    let mut out = String::new();
    out.push_str("# agentsync authorized_keys\n");
    out.push_str("#\n");
    out.push_str("# One ssh-ed25519 public key per line. Lines starting with '#' are\n");
    out.push_str("# comments. Paste `agentsync key show` output from any device you\n");
    out.push_str("# want to authorize.\n\n");
    for p in peers {
        if p.label.is_empty() {
            out.push_str(&format!("{}\n", p.pubkey.to_ssh_string()));
        } else {
            out.push_str(&format!("{} {}\n", p.pubkey.to_ssh_string(), p.label));
        }
    }
    out
}

/// Backwards-compat alias.
pub fn render_peers_md(peers: &[AuthorizedPeer]) -> String {
    render_authorized_keys(peers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn parse_ssh_style_with_label() {
        let id = Identity::generate();
        let line = format!("{} alice", id.pubkey().to_ssh_string());
        let parsed = parse_authorized_keys(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, id.pubkey());
        assert_eq!(parsed[0].label, "alice");
    }

    #[test]
    fn parse_ssh_style_no_label() {
        let id = Identity::generate();
        let line = id.pubkey().to_ssh_string();
        let parsed = parse_authorized_keys(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "");
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let id = Identity::generate();
        let body = format!(
            "# top comment\n\n   # indented comment\n{} alice\n",
            id.pubkey().to_ssh_string()
        );
        let parsed = parse_authorized_keys(&body);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn accepts_legacy_markdown_bullet_form() {
        let id = Identity::generate();
        let line = format!("- `{}` — bob", id.pubkey().to_ssh_string());
        let parsed = parse_authorized_keys(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "bob");
    }

    #[test]
    fn render_round_trips() {
        let id = Identity::generate();
        let peers = vec![AuthorizedPeer {
            pubkey: id.pubkey(),
            label: "alice".into(),
        }];
        let rendered = render_authorized_keys(&peers);
        let parsed = parse_authorized_keys(&rendered);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, id.pubkey());
        assert_eq!(parsed[0].label, "alice");
    }

    #[test]
    fn render_emits_ssh_style_no_markdown() {
        let id = Identity::generate();
        let peers = vec![AuthorizedPeer {
            pubkey: id.pubkey(),
            label: "alice".into(),
        }];
        let body = render_authorized_keys(&peers);
        assert!(!body.contains("- `"), "must not use markdown bullets");
        assert!(body.contains("ssh-ed25519 "));
    }
}
