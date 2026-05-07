//! Crash recovery: write, drop the Vault, reopen, expect state preserved.
//! Plus: snapshot create/list/restore round-trip.

use agentsync_core::{BindOptions, CreateOptions, OpenOptions, Vault};
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn doc_bin_round_trip() {
    let dir = tempdir().unwrap();
    let storage = dir.path().join(".agentsync");
    let (vault, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: storage.clone(),
    })
    .await
    .unwrap();
    vault.write_text_file("a.md", "alpha").await.unwrap();
    vault.write_text_file("b.md", "bravo").await.unwrap();
    vault.flush().await.unwrap();
    drop(vault);

    let v2 = Vault::open(OpenOptions {
        rendezvous_url: None,
        vault_id: created.vault_id.clone(),
        identity: created.identity,
        storage_path: storage,
        hub_pubkey: None,
    })
    .await
    .unwrap();
    let paths = v2.list_file_paths().await.unwrap();
    assert!(paths.contains(&"a.md".to_string()));
    assert!(paths.contains(&"b.md".to_string()));
    assert_eq!(v2.read_text_file("a.md").await.unwrap(), "alpha");
}

#[tokio::test]
async fn snapshot_label_round_trip() {
    let dir = tempdir().unwrap();
    let (vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    vault.write_text_file("a.md", "v1").await.unwrap();
    vault.create_label("first").await.unwrap();
    vault.write_text_file("a.md", "v2").await.unwrap();
    vault.create_label("second").await.unwrap();
    let labels = vault.list_labels().await.unwrap();
    let names: Vec<_> = labels.iter().map(|l| l.name.clone()).collect();
    assert!(names.contains(&"first".to_string()));
    assert!(names.contains(&"second".to_string()));
}

#[tokio::test]
async fn restore_to_label_resets_content() {
    let dir = tempdir().unwrap();
    let (mut vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = vault
        .bind_directory(dir.path(), BindOptions::default())
        .await
        .unwrap();
    vault.write_text_file("a.md", "v1").await.unwrap();
    // Allow the binding to materialize.
    tokio::time::sleep(Duration::from_millis(200)).await;
    vault.create_label("snap-v1").await.unwrap();

    vault.write_text_file("a.md", "v2-much-changed").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(vault.read_text_file("a.md").await.unwrap(), "v2-much-changed");

    vault.restore_label("snap-v1").await.unwrap();
    assert_eq!(vault.read_text_file("a.md").await.unwrap(), "v1");
}

#[tokio::test]
async fn ingest_existing_files_on_bind() {
    // Files placed in the directory before binding must be picked up.
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("preexisting.md"), b"hello world")
        .await
        .unwrap();
    let storage = dir.path().join(".agentsync");
    let (mut vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: storage,
    })
    .await
    .unwrap();
    let _ = vault
        .bind_directory(dir.path(), BindOptions::default())
        .await
        .unwrap();
    // Initial scan happens during bind_directory.
    let content = vault.read_text_file("preexisting.md").await.unwrap();
    assert_eq!(content, "hello world");
}
