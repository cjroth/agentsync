//! In-process two-peer sync tests. One Vault listens, another connects.
//! Verifies that file writes propagate, deletes propagate, and concurrent
//! edits to the same file converge.

use agentsync_core::{BindOptions, CreateOptions, OpenOptions, Vault};
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

async fn read_disk(path: std::path::PathBuf) -> Option<String> {
    tokio::fs::read_to_string(&path).await.ok()
}

#[tokio::test]
async fn one_writer_one_reader_propagates() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agentsync_core=trace,warn")
        .try_init();

    // Server peer.
    let server_dir = tempdir().unwrap();
    let server_storage = server_dir.path().join(".agentsync");
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        vault_key: None,
        storage_path: server_storage.clone(),
    })
    .await
    .unwrap();
    let _binding = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("ws://{}", bound);

    // Client peer.
    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(url.clone()),
        vault_id: created.vault_id.clone(),
        vault_key: created.vault_key,
        storage_path: client_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _client_binding = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client.connect().await.unwrap();

    // Write on server, expect client to see it.
    server
        .write_text_file("hello.md", "hello from server")
        .await
        .unwrap();

    let client_path = client_dir.path().join("hello.md");
    let ok = wait_until(Duration::from_secs(5), || {
        let p = client_path.clone();
        async move {
            match read_disk(p).await {
                Some(s) => s == "hello from server",
                None => false,
            }
        }
    })
    .await;
    assert!(ok, "client never observed the file");

    // Write on client, expect server to see it.
    client
        .write_text_file("notes/from-client.md", "ack")
        .await
        .unwrap();
    let server_path = server_dir.path().join("notes/from-client.md");
    let ok = wait_until(Duration::from_secs(5), || {
        let p = server_path.clone();
        async move {
            match read_disk(p).await {
                Some(s) => s == "ack",
                None => false,
            }
        }
    })
    .await;
    assert!(ok, "server never observed client's file");

    // Delete on server, expect client to see deletion.
    server.delete_file("hello.md").await.unwrap();
    let ok = wait_until(Duration::from_secs(5), || {
        let p = client_dir.path().join("hello.md");
        async move { tokio::fs::metadata(&p).await.is_err() }
    })
    .await;
    assert!(ok, "client never observed deletion");
}

#[tokio::test]
async fn concurrent_edits_converge() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn")
        .try_init();

    let server_dir = tempdir().unwrap();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        vault_key: None,
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("ws://{}", bound);

    let make_client = |idx: u32| {
        let url = url.clone();
        let vault_id = created.vault_id.clone();
        async move {
            let dir = tempdir().unwrap();
            let mut v = Vault::open(OpenOptions {
                rendezvous_url: Some(url),
                vault_id,
                vault_key: created.vault_key,
                storage_path: dir.path().join(".agentsync"),
            })
            .await
            .unwrap();
            let _ = v
                .bind_directory(dir.path(), BindOptions::default())
                .await
                .unwrap();
            v.connect().await.unwrap();
            (v, dir, idx)
        }
    };

    let (v1, _d1, _) = make_client(1).await;
    let (v2, _d2, _) = make_client(2).await;

    // Both clients write to same file with different content.
    let f1 = v1.write_text_file("collab.md", "edit-from-1");
    let f2 = v2.write_text_file("collab.md", "edit-from-2");
    let _ = tokio::join!(f1, f2);

    // Wait for convergence on the server's view.
    let mut converged = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let v1_text = v1.read_text_file("collab.md").await.ok();
        let v2_text = v2.read_text_file("collab.md").await.ok();
        let s_text = server.read_text_file("collab.md").await.ok();
        if v1_text.is_some() && v1_text == v2_text && v2_text == s_text {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged, "peers did not converge on collab.md");
}

#[tokio::test]
async fn wrong_key_is_rejected() {
    let server_dir = tempdir().unwrap();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        vault_key: None,
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("ws://{}", bound);

    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(url),
        vault_id: created.vault_id.clone(),
        vault_key: [0u8; 32], // wrong key
        storage_path: client_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let res = client.connect().await;
    assert!(res.is_err(), "client with wrong key should be rejected");
}
