//! `.syncignore` (gitignore syntax) excludes files from the sync engine.
//! The file is read at bind time; nested files apply only within their
//! subtree; negation overrides earlier patterns.

use agentsync_core::{BindOptions, CreateOptions, Vault};
use std::time::Duration;
use tempfile::tempdir;

async fn open_and_list(dir: &std::path::Path) -> Vec<String> {
    let (mut vault, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: dir.join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = vault
        .bind_directory(dir, BindOptions::default())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut paths = vault.list_file_paths().await.unwrap();
    paths.sort();
    paths
}

/// Same as `open_and_list` but drops the engine-managed files
/// (`authorized_keys` and any `.syncignore`) so a test can assert about the
/// user's content alone.
async fn open_and_list_user_files(dir: &std::path::Path) -> Vec<String> {
    let mut paths = open_and_list(dir).await;
    paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
    paths
}

#[tokio::test]
async fn syncignore_itself_is_synced_under_markdown_default() {
    // `.syncignore` is the shared exclusion policy — it must propagate to
    // peers regardless of the include filter (default is `**/*.md`, which
    // would otherwise exclude an extension-less file).
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"*.tmp\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"hi")
        .await
        .unwrap();

    let paths = open_and_list(dir.path()).await;
    assert!(
        paths.contains(&".syncignore".to_string()),
        ".syncignore should be ingested into the doc, got: {:?}",
        paths
    );
}

#[tokio::test]
async fn nested_syncignore_is_also_synced() {
    // Nested `.syncignore` files have their own rules anchored to their dir,
    // so they too must reach peers.
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join("sub")).await.unwrap();
    tokio::fs::write(dir.path().join("sub/.syncignore"), b"*.log\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("sub/keep.md"), b"a")
        .await
        .unwrap();

    let paths = open_and_list(dir.path()).await;
    assert!(
        paths.contains(&"sub/.syncignore".to_string()),
        "nested .syncignore should be ingested, got: {:?}",
        paths
    );
}

#[tokio::test]
async fn syncignore_cannot_hide_itself() {
    // A user adding `.syncignore` to its own contents must not cause the
    // file to disappear from sync — without that guarantee, peers could
    // diverge on which exclusion rules they apply.
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b".syncignore\n*.tmp\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"hi")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("scratch.tmp"), b"x")
        .await
        .unwrap();

    let paths = open_and_list(dir.path()).await;
    assert!(
        paths.contains(&".syncignore".to_string()),
        ".syncignore must sync even if it lists itself, got: {:?}",
        paths
    );
    assert!(
        !paths.contains(&"scratch.tmp".to_string()),
        "*.tmp rule still applies to other files, got: {:?}",
        paths
    );
}

#[tokio::test]
async fn syncignore_excludes_matching_files() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"secret.md\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("public.md"), b"a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("secret.md"), b"shh")
        .await
        .unwrap();

    let paths = open_and_list_user_files(dir.path()).await;
    assert_eq!(paths, vec!["public.md".to_string()]);
}

#[tokio::test]
async fn syncignore_directory_pattern_excludes_subtree() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"build/\n")
        .await
        .unwrap();
    tokio::fs::create_dir_all(dir.path().join("build")).await.unwrap();
    tokio::fs::write(dir.path().join("build/out.md"), b"x")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("keep.md"), b"y")
        .await
        .unwrap();

    let paths = open_and_list_user_files(dir.path()).await;
    assert_eq!(paths, vec!["keep.md".to_string()]);
}

#[tokio::test]
async fn syncignore_negation_keeps_specific_file() {
    let dir = tempdir().unwrap();
    tokio::fs::write(
        dir.path().join(".syncignore"),
        b"*.log.md\n!keep.log.md\n",
    )
    .await
    .unwrap();
    tokio::fs::write(dir.path().join("debug.log.md"), b"a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("keep.log.md"), b"b")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"c")
        .await
        .unwrap();

    let paths = open_and_list_user_files(dir.path()).await;
    assert_eq!(
        paths,
        vec!["keep.log.md".to_string(), "note.md".to_string()]
    );
}

#[tokio::test]
async fn syncignore_edits_after_bind_take_effect() {
    // Regression: editing `.syncignore` while `agentsync watch` is already
    // running must update the live filter. Previously the matcher set was
    // built once in `Binding::new` and never refreshed, so a pattern added
    // mid-session leaked through and the matching file synced anyway.
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"keep")
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
    tokio::time::sleep(Duration::from_millis(200)).await;

    // User edits `.syncignore` mid-session.
    tokio::fs::write(dir.path().join(".syncignore"), b"hello*\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A file matching the new pattern must NOT be ingested.
    tokio::fs::write(dir.path().join("hello.md"), b"world")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
    paths.sort();
    assert_eq!(
        paths,
        vec!["note.md".to_string()],
        "hello.md should be excluded by the live-edited .syncignore"
    );
}

#[tokio::test]
async fn syncignore_created_after_bind_takes_effect() {
    // Same bug, slightly different shape: `.syncignore` doesn't exist when
    // the binding starts. Creating it later must register the matchers so
    // newly-matching files are excluded.
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
    tokio::time::sleep(Duration::from_millis(200)).await;

    tokio::fs::write(dir.path().join(".syncignore"), b"secret*\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::fs::write(dir.path().join("secret.md"), b"shh")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("public.md"), b"ok")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
    paths.sort();
    assert_eq!(paths, vec!["public.md".to_string()]);
}

#[tokio::test]
async fn relaxing_syncignore_rule_ingests_previously_excluded_file() {
    // Regression: removing a pattern (or deleting `.syncignore` outright)
    // must cause already-on-disk files that used to match the rule to be
    // ingested. Otherwise users have to bounce `agentsync watch` to recover
    // a file they meant to start syncing.
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"secret*\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("secret.md"), b"shh")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"hi")
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
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Sanity: secret.md is excluded right now.
    {
        let mut paths = vault.list_file_paths().await.unwrap();
        paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
        paths.sort();
        assert_eq!(paths, vec!["note.md".to_string()]);
    }

    // User relaxes the rule. The on-disk secret.md is unchanged, but it
    // should now ingest into the doc.
    tokio::fs::write(dir.path().join(".syncignore"), b"")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
    paths.sort();
    assert_eq!(
        paths,
        vec!["note.md".to_string(), "secret.md".to_string()],
        "relaxing the rule should have ingested secret.md"
    );
}

#[tokio::test]
async fn deleting_syncignore_ingests_previously_excluded_file() {
    // Removing `.syncignore` entirely is a stronger form of relaxing it —
    // every previously-excluded file should now appear in the doc.
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join(".syncignore"), b"hello*\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("hello.md"), b"world")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("note.md"), b"hi")
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
    tokio::time::sleep(Duration::from_millis(200)).await;

    tokio::fs::remove_file(dir.path().join(".syncignore"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut paths = vault.list_file_paths().await.unwrap();
    paths.retain(|p| p != "authorized_keys" && !p.ends_with(".syncignore"));
    paths.sort();
    assert_eq!(
        paths,
        vec!["hello.md".to_string(), "note.md".to_string()]
    );
}

#[tokio::test]
async fn nested_syncignore_only_applies_in_its_subtree() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join("sub")).await.unwrap();
    tokio::fs::write(dir.path().join("sub/.syncignore"), b"hidden.md\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("hidden.md"), b"a") // not excluded at root
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("sub/hidden.md"), b"b") // excluded
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("sub/visible.md"), b"c")
        .await
        .unwrap();

    let paths = open_and_list_user_files(dir.path()).await;
    assert_eq!(
        paths,
        vec!["hidden.md".to_string(), "sub/visible.md".to_string()]
    );
}
