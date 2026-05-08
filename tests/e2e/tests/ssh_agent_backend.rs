//! Phase 3 — ssh-agent identity backend.
//!
//! Verifies signing-by-agent end to end: a peer that uses an agent-backed
//! identity successfully completes the handshake against a hub that uses a
//! file-backed identity, with the resulting WSS connection syncing both
//! ways.

use agentsync_core::{
    agent::agent_list_identities_at, BindOptions, CreateOptions, Identity, OpenOptions,
    Pubkey, Vault,
};
use agentsync_e2e::{authorize_in_process, MockAgent};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn agent_lists_identities() {
    let signing = SigningKey::generate(&mut OsRng);
    let agent = MockAgent::start(signing.clone()).await.unwrap();

    let identities = agent_list_identities_at(&agent.socket_path).await.unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0], Pubkey(signing.verifying_key().to_bytes()));
}

/// File-backed hub + agent-backed peer sync over WSS. Exercises the full
/// signing path through the agent socket.
#[tokio::test]
async fn agent_backed_peer_syncs_with_file_backed_hub() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn")
        .try_init();

    // Hub: ordinary file-backed identity.
    let server_dir = tempdir().unwrap();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("wss://{}", bound);

    // Peer: ed25519 key held by a mock ssh-agent.
    let signing = SigningKey::generate(&mut OsRng);
    let peer_pubkey = Pubkey(signing.verifying_key().to_bytes());
    let agent = MockAgent::start(signing).await.unwrap();
    let identity = Identity::from_agent(agent.socket_path.clone(), peer_pubkey);

    authorize_in_process(&server, "agent-peer", &peer_pubkey).await;

    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(url),
        vault_id: created.vault_id.clone(),
        identity,
        storage_path: client_dir.path().join(".agentsync"),
        hub_pubkey: None,
        name: None,
    })
    .await
    .unwrap();
    let _ = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client.connect().await.unwrap();

    // Push a file from the hub; expect the agent-backed peer to receive it.
    server
        .write_text_file("via-agent.md", "hello over ssh-agent")
        .await
        .unwrap();

    let path = client_dir.path().join("via-agent.md");
    let mut ok = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(s) = tokio::fs::read_to_string(&path).await {
            if s == "hello over ssh-agent" {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "agent-backed peer never received the hub's write");
}

/// An agent-backed identity that asks the agent to sign with a pubkey the
/// agent doesn't hold gets a clear error before any signing is attempted.
#[tokio::test]
async fn agent_with_missing_pubkey_errors_clearly() {
    let signing = SigningKey::generate(&mut OsRng);
    let agent = MockAgent::start(signing).await.unwrap();
    // Build an identity whose pubkey is NOT what the agent holds.
    let other = SigningKey::generate(&mut OsRng);
    let other_pubkey = Pubkey(other.verifying_key().to_bytes());
    let identity = Identity::from_agent(agent.socket_path.clone(), other_pubkey);

    let err = identity
        .sign(b"transcript")
        .await
        .expect_err("sign should have failed");
    let msg = err.to_string();
    assert!(
        msg.contains("no key matching"),
        "expected 'no key matching' error, got: {}",
        msg
    );
}
