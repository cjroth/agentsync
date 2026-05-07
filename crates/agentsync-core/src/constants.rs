//! Centralized defaults shared between the CLI and the core library.

/// Default rendezvous port. Used when `--listen` is given without a value
/// and when a `--rendezvous` URL is supplied without an explicit port.
pub const DEFAULT_PORT: u16 = 1234;

/// Default `--listen` bind address (`0.0.0.0:<DEFAULT_PORT>`).
pub const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:1234";

/// Filename of the authorized-keys list inside a vault. SSH-style format:
/// one `ssh-ed25519 <base64> [comment]` per line, `#` comments allowed.
pub const AUTHORIZED_KEYS_FILE: &str = "authorized_keys";

/// Per-user state directory (relative to `$HOME`). Holds the default
/// identity keypair shared across vaults.
pub const USER_STATE_DIR: &str = ".agentsync";

/// Default identity filename inside [`USER_STATE_DIR`]. Mirrors SSH's
/// `~/.ssh/id_ed25519` so users carry the same convention over.
pub const USER_IDENTITY_FILENAME: &str = "id_ed25519";

/// Normalize a rendezvous URL: if it has no explicit port, append
/// `:DEFAULT_PORT`. Anything else is returned unchanged. Invalid URLs are
/// returned unchanged so the caller's parser produces the canonical error
/// message.
pub fn normalize_rendezvous_url(url: &str) -> String {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };
    if parsed.port().is_some() {
        return url.to_string();
    }
    if parsed.scheme() != "ws" && parsed.scheme() != "wss" {
        return url.to_string();
    }
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return url.to_string(),
    };
    // Reconstruct without depending on Url::set_port, which rejects
    // default-port schemes (it considers wss == 443).
    let mut out = format!("{}://{}:{}", parsed.scheme(), host, DEFAULT_PORT);
    let path = parsed.path();
    if !path.is_empty() && path != "/" {
        out.push_str(path);
    }
    if let Some(q) = parsed.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = parsed.fragment() {
        out.push('#');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_default_port_when_missing() {
        assert_eq!(
            normalize_rendezvous_url("wss://127.0.0.1"),
            "wss://127.0.0.1:1234"
        );
        assert_eq!(
            normalize_rendezvous_url("ws://example.com"),
            "ws://example.com:1234"
        );
    }

    #[test]
    fn keeps_explicit_port() {
        assert_eq!(
            normalize_rendezvous_url("wss://127.0.0.1:9999"),
            "wss://127.0.0.1:9999"
        );
    }

    #[test]
    fn preserves_path_and_query() {
        assert_eq!(
            normalize_rendezvous_url("wss://h.example/api?x=1"),
            "wss://h.example:1234/api?x=1"
        );
    }

    #[test]
    fn passes_through_invalid() {
        assert_eq!(normalize_rendezvous_url("not-a-url"), "not-a-url");
    }
}
