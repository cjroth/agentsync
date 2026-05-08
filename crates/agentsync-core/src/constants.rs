//! Centralized defaults shared between the CLI and the core library.

/// Default `--listen` bind port. Matches the standard `wss://` scheme port
/// so URLs can elide the port (`wss://my-hub`) and most corporate
/// firewalls / hotel wifi / mobile carriers — which permit 443 outbound —
/// let the connection through. Privileged on Unix: hubs running as a
/// regular user need `setcap cap_net_bind_service=+ep` on the binary
/// (Linux) or socket activation / a launchd dropper (macOS) to bind it.
pub const DEFAULT_PORT: u16 = 443;

/// Default `--listen` bind address (`0.0.0.0:<DEFAULT_PORT>`).
pub const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:443";

/// Default `--listen` bind address when TLS is disabled (`0.0.0.0:80`),
/// matching the `ws://` scheme default. Used for `--listen --no-tls`
/// deployments where a reverse proxy (Fly.io, Railway) terminates TLS at
/// the edge and forwards plain HTTP/WS to the hub.
pub const DEFAULT_LISTEN_ADDR_NO_TLS: &str = "0.0.0.0:80";

/// Filename of the authorized-keys list inside a vault. SSH-style format:
/// one `ssh-ed25519 <base64> [comment]` per line, `#` comments allowed.
pub const AUTHORIZED_KEYS_FILE: &str = "authorized_keys";

/// Per-user state directory (relative to `$HOME`). Holds the default
/// identity keypair shared across vaults.
pub const USER_STATE_DIR: &str = ".agentsync";

/// Default identity filename inside [`USER_STATE_DIR`]. Mirrors SSH's
/// `~/.ssh/id_ed25519` so users carry the same convention over.
pub const USER_IDENTITY_FILENAME: &str = "id_ed25519";

/// Add a default scheme to a rendezvous URL when none is given. Inputs
/// like `my-hub` or `my-hub:8443` get `wss://` (or `ws://` for `no_tls`)
/// prepended; inputs that already specify a scheme are returned as-is.
///
/// The WebSocket client uses the scheme-default port (443 for `wss`, 80
/// for `ws`) when none is specified, which matches typical reverse-proxy
/// deployments (Fly.io, Railway, etc.) and the local `--listen` default.
pub fn normalize_rendezvous_url(url: &str) -> String {
    normalize_with_scheme(url, false)
}

/// Variant of [`normalize_rendezvous_url`] used when the caller has
/// `--no-tls` set: missing schemes default to `ws://` instead of `wss://`.
pub fn normalize_with_scheme(url: &str, no_tls: bool) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        return trimmed.to_string();
    }
    if has_scheme_prefix(trimmed) {
        // Some other scheme — leave it alone so the parser can reject it
        // with a useful error.
        return trimmed.to_string();
    }
    let scheme = if no_tls { "ws" } else { "wss" };
    format!("{}://{}", scheme, trimmed)
}

/// True if `s` starts with a `<scheme>://` prefix.
fn has_scheme_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let alpha = c.is_ascii_alphabetic();
        let digit = c.is_ascii_digit();
        let allowed = matches!(c, b'+' | b'-' | b'.');
        if i == 0 {
            if !alpha {
                return false;
            }
        } else if !(alpha || digit || allowed) {
            return s[i..].starts_with("://");
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_preserves_explicit_scheme() {
        assert_eq!(
            normalize_rendezvous_url("wss://127.0.0.1"),
            "wss://127.0.0.1"
        );
        assert_eq!(
            normalize_rendezvous_url("wss://127.0.0.1:9999"),
            "wss://127.0.0.1:9999"
        );
        assert_eq!(
            normalize_rendezvous_url("wss://h.example/api?x=1"),
            "wss://h.example/api?x=1"
        );
        assert_eq!(
            normalize_rendezvous_url("ws://my-hub:8080"),
            "ws://my-hub:8080"
        );
    }

    #[test]
    fn missing_scheme_defaults_to_wss() {
        assert_eq!(normalize_rendezvous_url("my-hub"), "wss://my-hub");
        assert_eq!(
            normalize_rendezvous_url("my-hub:8443"),
            "wss://my-hub:8443"
        );
        assert_eq!(
            normalize_rendezvous_url("hub.example.com"),
            "wss://hub.example.com"
        );
        assert_eq!(
            normalize_rendezvous_url("127.0.0.1:9999"),
            "wss://127.0.0.1:9999"
        );
    }

    #[test]
    fn no_tls_defaults_to_ws() {
        assert_eq!(normalize_with_scheme("my-hub", true), "ws://my-hub");
        assert_eq!(
            normalize_with_scheme("my-hub:8080", true),
            "ws://my-hub:8080"
        );
        // Explicit schemes still win.
        assert_eq!(
            normalize_with_scheme("wss://my-hub", true),
            "wss://my-hub"
        );
    }

    #[test]
    fn other_scheme_left_alone() {
        // Not a ws scheme — pass through untouched so the parser can
        // produce a meaningful error.
        assert_eq!(
            normalize_rendezvous_url("http://h.example"),
            "http://h.example"
        );
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(
            normalize_rendezvous_url("  my-hub  "),
            "wss://my-hub"
        );
    }
}
