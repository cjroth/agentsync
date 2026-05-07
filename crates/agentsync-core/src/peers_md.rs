//! Parser for `peers.md` — the markdown file at the root of a vault that
//! lists authorized device pubkeys.
//!
//! The format is intentionally permissive: lines that look like
//! `- ` ssh-ed25519 <base64> ` — <label>` are treated as authorized peers,
//! and everything else is ignored as freeform notes. Editors can paste
//! `~/.ssh/id_ed25519.pub` lines straight in.

use crate::identity::Pubkey;

pub const PEERS_FILE: &str = "peers.md";

#[derive(Debug, Clone)]
pub struct AuthorizedPeer {
    pub pubkey: Pubkey,
    pub label: String,
}

/// Parse `peers.md` content. Unparseable lines are silently skipped — they
/// are assumed to be human-only commentary.
pub fn parse_peers_md(content: &str) -> Vec<AuthorizedPeer> {
    let mut out = Vec::new();
    for raw in content.lines() {
        if let Some(peer) = parse_line(raw) {
            out.push(peer);
        }
    }
    out
}

fn parse_line(raw: &str) -> Option<AuthorizedPeer> {
    let line = raw.trim_start_matches(|c: char| c.is_whitespace());
    let after_dash = strip_list_marker(line)?;
    let (key_part, label) = split_key_and_label(after_dash);
    let key_text = key_part.trim().trim_matches('`').trim();
    let pk = Pubkey::from_ssh_string(key_text).ok()?;
    Some(AuthorizedPeer {
        pubkey: pk,
        label: label.to_string(),
    })
}

fn strip_list_marker(line: &str) -> Option<&str> {
    // Accept `- ` and `* ` markdown list markers.
    for marker in ["- ", "* "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

/// Split the line at the label separator. Accepts em-dash, en-dash, or two
/// hyphens, optionally surrounded by whitespace. The label is the trimmed
/// remainder; if no separator is present, the label is empty.
fn split_key_and_label(s: &str) -> (&str, &str) {
    for sep in [" — ", " – ", " -- ", " - "] {
        if let Some(idx) = s.find(sep) {
            let (k, rest) = s.split_at(idx);
            return (k, rest[sep.len()..].trim());
        }
    }
    (s, "")
}

/// Render a list of authorized peers as canonical `peers.md` content, with a
/// header. Used by `agentsync init` to seed a new vault.
pub fn render_peers_md(peers: &[AuthorizedPeer]) -> String {
    let mut out = String::new();
    out.push_str("# Authorized peers\n\n");
    out.push_str(
        "Lines matching `- \\`ssh-ed25519 <base64>\\` — <label>` are parsed.\n\
         Everything else is ignored — feel free to add freeform notes.\n\n",
    );
    for p in peers {
        out.push_str(&format!(
            "- `{}` — {}\n",
            p.pubkey.to_ssh_string(),
            if p.label.is_empty() { "(unlabeled)" } else { &p.label }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn parse_basic() {
        let id = Identity::generate();
        let line = format!("- `{}` — alice", id.pubkey().to_ssh_string());
        let parsed = parse_peers_md(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, id.pubkey());
        assert_eq!(parsed[0].label, "alice");
    }

    #[test]
    fn ignores_freeform_text() {
        let content = "# Heading\n\nSome notes here.\n\n- not a key\n";
        assert!(parse_peers_md(content).is_empty());
    }

    #[test]
    fn parses_without_backticks() {
        let id = Identity::generate();
        let line = format!("- {} — bob", id.pubkey().to_ssh_string());
        let parsed = parse_peers_md(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "bob");
    }

    #[test]
    fn parses_double_dash_separator() {
        let id = Identity::generate();
        let line = format!("- `{}` -- carol (hub)", id.pubkey().to_ssh_string());
        let parsed = parse_peers_md(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "carol (hub)");
    }

    #[test]
    fn parses_no_label() {
        let id = Identity::generate();
        let line = format!("- `{}`", id.pubkey().to_ssh_string());
        let parsed = parse_peers_md(&line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "");
    }

    #[test]
    fn render_round_trips() {
        let id = Identity::generate();
        let peers = vec![AuthorizedPeer {
            pubkey: id.pubkey(),
            label: "alice".into(),
        }];
        let rendered = render_peers_md(&peers);
        let parsed = parse_peers_md(&rendered);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, id.pubkey());
        assert_eq!(parsed[0].label, "alice");
    }
}
