//! End-to-end harness that spawns the real `agentsync` CLI binary.
//!
//! Each `E2EVault` is a self-contained scenario:
//!   * `rendezvous` — a `--listen` peer in a fresh tempdir.
//!   * `peers`      — additional `agentsync` processes connected to it.
//!
//! Tests drive the peers via `Peer::save_atomic` / `Peer::save_truncate` /
//! `Peer::delete` and assert with `wait_for_content` / `wait_for_missing`.
//!
//! The harness builds the binary on first use (cached) and spawns it with
//! `kill_on_drop`, so dropped scenarios always clean up their child processes.

pub mod harness;
pub mod mock_agent;

pub use harness::{E2EVault, Peer};
pub use mock_agent::MockAgent;

use agentsync_core::{Pubkey, Vault};

/// Append a peer's pubkey to the in-memory peers.md of an in-process vault.
/// Used by the in-process integration tests that don't go through the CLI
/// harness — those that do should call [`E2EVault::authorize_peer`] instead.
pub async fn authorize_in_process(vault: &Vault, label: &str, pk: &Pubkey) {
    let cur = vault.read_text_file("peers.md").await.unwrap_or_default();
    let line = format!("- `{}` — {}\n", pk.to_ssh_string(), label);
    let updated = if cur.ends_with('\n') || cur.is_empty() {
        format!("{}{}", cur, line)
    } else {
        format!("{}\n{}", cur, line)
    };
    vault
        .write_text_file("peers.md", &updated)
        .await
        .expect("write peers.md");
}
