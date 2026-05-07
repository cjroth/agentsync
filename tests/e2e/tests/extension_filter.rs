//! Defaults restrict ingestion to markdown only; configuring extra extensions
//! opts other types in.

use agentsync_core::{BindOptions, CreateOptions, Vault};
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn default_skips_non_markdown_files() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"keep me")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("data.json"), b"{}")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("script.py"), b"print(1)")
        .await
        .unwrap();

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
    // Allow the periodic materializer to settle.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.sort();
    // peers.md is auto-seeded by `Vault::create`; ignore it for this assert.
    paths.retain(|p| p != "peers.md");
    assert_eq!(paths, vec!["note.md".to_string()]);
}

#[tokio::test]
async fn extending_text_extensions_opts_in_more_files() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("data.json"), b"{}")
        .await
        .unwrap();

    let (mut vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let opts = BindOptions::for_extensions(["md", "json"]);
    let _ = vault.bind_directory(dir.path(), opts).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "peers.md");
    paths.sort();
    assert_eq!(
        paths,
        vec!["data.json".to_string(), "note.md".to_string()]
    );
    // .json should be ingested as text (its content should be readable as a
    // string, not stored as an attachment).
    let body = vault.read_text_file("data.json").await.unwrap();
    assert_eq!(body, "{}");
}

#[tokio::test]
async fn explicit_size_limit_is_respected() {
    let dir = tempdir().unwrap();
    let big = "x".repeat(2048);
    tokio::fs::write(dir.path().join("big.md"), big.as_bytes())
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("ok.md"), b"hi")
        .await
        .unwrap();

    let (mut vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let mut opts = BindOptions::default();
    opts.text_file_max_bytes = 1024;
    let _ = vault.bind_directory(dir.path(), opts).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "peers.md");
    paths.sort();
    assert_eq!(paths, vec!["ok.md".to_string()]);
}
