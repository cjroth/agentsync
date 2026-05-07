//! Folder sync regressions: empty directory creation, empty directory
//! deletion, and `rm -rf` on a populated tree all need to propagate between
//! peers — previously only file-level events did.

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

/// Set up a server-and-client Vault pair, both bound to their own temp dirs,
/// connected over a localhost websocket. Returns the pair and their dirs so
/// callers can poke at the disk directly.
async fn make_pair() -> (Vault, tempfile::TempDir, Vault, tempfile::TempDir) {
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
    let bound = server
        .listen("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let url = format!("ws://{}", bound);

    let client_dir = tempdir().unwrap();
    let mut client = Vault::open(OpenOptions {
        rendezvous_url: Some(url),
        vault_id: created.vault_id.clone(),
        vault_key: created.vault_key,
        storage_path: client_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = client
        .bind_directory(client_dir.path(), BindOptions::default())
        .await
        .unwrap();
    client.connect().await.unwrap();
    // Give the initial sync round a moment to settle before the test starts
    // poking the filesystem.
    tokio::time::sleep(Duration::from_millis(300)).await;

    (server, server_dir, client, client_dir)
}

#[tokio::test]
async fn empty_folder_create_propagates() {
    let (server, server_dir, _client, client_dir) = make_pair().await;

    // Create a brand-new empty folder on the server side.
    let new_dir = server_dir.path().join("ideas");
    std::fs::create_dir(&new_dir).unwrap();

    let target = client_dir.path().join("ideas");
    let appeared = wait_until(Duration::from_secs(5), || {
        let p = target.clone();
        async move { tokio::fs::metadata(&p).await.map(|m| m.is_dir()).unwrap_or(false) }
    })
    .await;
    assert!(
        appeared,
        "client never observed the empty folder {:?}",
        target
    );

    // Sanity: the doc on each side records the directory.
    let server_dirs = server.list_directories().await.unwrap();
    assert!(server_dirs.iter().any(|d| d.path == "ideas"));
}

#[tokio::test]
async fn empty_folder_delete_propagates() {
    let (server, server_dir, client, client_dir) = make_pair().await;

    // Create on server, wait for client to observe it, then delete on
    // server and assert client removes it too.
    std::fs::create_dir(server_dir.path().join("scratch")).unwrap();
    let client_path = client_dir.path().join("scratch");
    let appeared = wait_until(Duration::from_secs(5), || {
        let p = client_path.clone();
        async move { tokio::fs::metadata(&p).await.map(|m| m.is_dir()).unwrap_or(false) }
    })
    .await;
    assert!(appeared, "client never saw scratch/");

    std::fs::remove_dir(server_dir.path().join("scratch")).unwrap();

    let gone = wait_until(Duration::from_secs(5), || {
        let p = client_path.clone();
        async move { tokio::fs::metadata(&p).await.is_err() }
    })
    .await;
    assert!(gone, "client still has scratch/ after server removed it");

    // The doc should reflect the tombstone too.
    let live_dirs = client.list_directories().await.unwrap();
    assert!(
        !live_dirs.iter().any(|d| d.path == "scratch"),
        "client doc still lists scratch/: {:?}",
        live_dirs
    );
}

#[tokio::test]
async fn rm_rf_populated_folder_propagates() {
    let (_server, server_dir, _client, client_dir) = make_pair().await;

    // Create a folder containing a file on the server.
    let dir = server_dir.path().join("notes");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "hello").unwrap();
    std::fs::write(dir.join("b.md"), "world").unwrap();

    let client_dir_path = client_dir.path().join("notes");
    let client_a = client_dir_path.join("a.md");
    let appeared = wait_until(Duration::from_secs(5), || {
        let p = client_a.clone();
        async move { tokio::fs::metadata(&p).await.is_ok() }
    })
    .await;
    assert!(appeared, "client never saw notes/a.md");

    // rm -rf the folder on the server.
    std::fs::remove_dir_all(&dir).unwrap();

    let gone = wait_until(Duration::from_secs(5), || {
        let p = client_dir_path.clone();
        async move { tokio::fs::metadata(&p).await.is_err() }
    })
    .await;
    assert!(gone, "client still has notes/ after server rm -rf");
}

#[tokio::test]
async fn nested_empty_folder_create_propagates() {
    let (_server, server_dir, _client, client_dir) = make_pair().await;

    std::fs::create_dir_all(server_dir.path().join("a/b/c")).unwrap();

    let target = client_dir.path().join("a/b/c");
    let appeared = wait_until(Duration::from_secs(5), || {
        let p = target.clone();
        async move { tokio::fs::metadata(&p).await.map(|m| m.is_dir()).unwrap_or(false) }
    })
    .await;
    assert!(appeared, "client never observed nested folder a/b/c");

    // All three intermediate dirs should also exist on the client.
    for sub in ["a", "a/b"] {
        let p = client_dir.path().join(sub);
        assert!(tokio::fs::metadata(&p).await.map(|m| m.is_dir()).unwrap_or(false),
            "client missing intermediate {}", sub);
    }
}
