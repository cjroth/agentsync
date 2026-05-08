//! Centralized defaults shared between the CLI and the core library.

/// Default `--listen` bind port. Unprivileged so any user can run a hub
/// without `CAP_NET_BIND_SERVICE`.
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

/// Currently a passthrough — the WebSocket client uses the scheme-default
/// port (443 for `wss`, 80 for `ws`) when none is specified, which matches
/// typical reverse-proxy deployments (Fly.io, Railway, etc.). Self-hosted
/// hubs running on the unprivileged `--listen` default ([`DEFAULT_PORT`])
/// must include the port explicitly in their rendezvous URL.
pub fn normalize_rendezvous_url(url: &str) -> String {
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_preserves_input() {
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
        assert_eq!(normalize_rendezvous_url("not-a-url"), "not-a-url");
    }
}
