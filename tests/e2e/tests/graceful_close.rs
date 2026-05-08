//! Verify that `Vault::disconnect` on a client cleanly tears down the
//! websocket on both ends — the server's peer slot for that connection should
//! be released, and the call itself should not hang waiting for tasks.

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
async fn client_disconnect_releases_server_peer() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn")
        .try_init();

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
        .listen("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let url = format!("wss://{}", bound);

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
    let _ = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client.connect().await.unwrap();

    // Push something across so we know the connection is fully alive.
    server.write_text_file("hello.md", "hi").await.unwrap();
    let client_path = client_dir.path().join("hello.md");
    let appeared = wait_until(Duration::from_secs(5), || {
        let p = client_path.clone();
        async move { tokio::fs::metadata(&p).await.is_ok() }
    })
    .await;
    assert!(appeared, "client never saw hello.md");

    assert_eq!(server.peer_count().await, 1, "server should see one peer");

    // Graceful disconnect. Should return promptly (under 2s — close path has
    // an internal timeout, but a clean Close exchange is far faster).
    let disconnect = tokio::time::timeout(Duration::from_secs(3), client.disconnect());
    disconnect.await.expect("disconnect did not complete in time");

    // Server should drop its peer slot once the websocket close has been
    // exchanged. Allow a moment for the close frame to round-trip.
    let cleared = wait_until(Duration::from_secs(3), || async {
        server.peer_count().await == 0
    })
    .await;
    assert!(
        cleared,
        "server still holds a peer slot after client disconnect"
    );
}

#[tokio::test]
async fn server_unlisten_releases_client_peer() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn")
        .try_init();

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
        .listen("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let url = format!("wss://{}", bound);

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
    let _ = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client.connect().await.unwrap();

    // Wait for handshake to settle.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.peer_count().await, 1);
    assert_eq!(client.peer_count().await, 1);

    let unlisten = tokio::time::timeout(Duration::from_secs(3), server.unlisten());
    unlisten.await.expect("unlisten did not complete in time");

    let cleared = wait_until(Duration::from_secs(3), || async {
        client.peer_count().await == 0
    })
    .await;
    assert!(cleared, "client still holds a peer slot after server unlisten");
}
