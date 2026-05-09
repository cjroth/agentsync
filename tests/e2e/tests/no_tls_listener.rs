//! `--no-tls` plain-WS listener: the hub binds plain TCP, peers connect
//! over `ws://` without TLS. Channel binding degrades to a constant zero
//! fingerprint (covered by the handshake transcript on both sides), so
//! the hub-identity signature is what authenticates the connection.

use agentsync_core::{BindOptions, CreateOptions, Identity, OpenOptions, Vault};
use agentsync_e2e::authorize_in_process;
use std::time::Duration;
use tempfile::tempdir;

async fn wait_until<F, Fut>(timeout: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn plain_ws_round_trip() {
    let server_dir = tempdir().unwrap();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _binding = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    // Bind without TLS — plain WebSocket on a local port.
    let bound = server
        .listen_plain("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let url = format!("ws://{}", bound);

    let client_identity = Identity::generate();
    authorize_in_process(&server, "client", &client_identity.pubkey()).await;

    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(url),
        vault_id: created.vault_id.clone(),
        identity: client_identity,
        storage_path: client_dir.path().join(".agentsync"),
        hub_pubkey: None,
        name: None,
    })
    .await
    .unwrap();
    let _client_binding = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client
        .connect()
        .await
        .expect("plain ws connect should succeed");

    server
        .write_text_file("plain.md", "hello over plain ws")
        .await
        .unwrap();
    let target = client_dir.path().join("plain.md");
    let synced =
        wait_until(Duration::from_secs(5), || {
            let p = target.clone();
            async move {
                tokio::fs::read_to_string(&p).await.ok().as_deref() == Some("hello over plain ws")
            }
        })
        .await;
    assert!(synced, "client never received the file over ws://");
}

/// `wss://` clients must still fail when pointed at a plain hub — the TLS
/// handshake has nothing to talk to. This pins the diagnostic so a
/// misconfiguration produces a recognisable transport-level error rather
/// than a silent hang.
#[tokio::test]
async fn wss_client_against_plain_hub_fails() {
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
    let bound = server
        .listen_plain("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let wss_url = format!("wss://{}", bound);

    let client_identity = Identity::generate();
    authorize_in_process(&server, "client", &client_identity.pubkey()).await;

    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(wss_url),
        vault_id: created.vault_id.clone(),
        identity: client_identity,
        storage_path: client_dir.path().join(".agentsync"),
        hub_pubkey: None,
        name: None,
    })
    .await
    .unwrap();

    let res = tokio::time::timeout(Duration::from_secs(5), client.connect()).await;
    let inner = res.expect("connect should not hang");
    assert!(
        inner.is_err(),
        "wss:// client somehow succeeded against plain hub"
    );
}
