//! End-to-end tests that exercise the real `agentsync` binary in subprocesses.
//!
//! These tests are slower than the in-process integration tests
//! (each spawns 2+ processes and waits for filesystem propagation) but they
//! are the only tests that catch real-world editor save patterns and CLI
//! behavior.

use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[tokio::test]
async fn atomic_save_round_trip() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0)
        .save_atomic("note.md", "hello via atomic save")
        .unwrap();
    v.rendezvous
        .wait_for_content("note.md", "hello via atomic save", T)
        .await
        .unwrap();

    v.rendezvous.save_atomic("from-server.md", "world").unwrap();
    v.peer(0)
        .wait_for_content("from-server.md", "world", T)
        .await
        .unwrap();

    v.shutdown().await;
}

/// Regression test for the user-reported bug: editors that truncate-then-write
/// previously caused both peers to end up with the file emptied. Debouncing
/// fs events fixes this.
#[tokio::test]
async fn truncate_save_does_not_lose_content() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Pre-populate with v1 so the bug surface ("transient empty overwrites
    // existing content") is exercised.
    v.peer(0).save_atomic("yo.md", "version-1").unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "version-1", T)
        .await
        .unwrap();

    // Now save via truncate-then-write. The harness writes empty, sleeps
    // 40ms, then writes the new content — same shape as a slow editor.
    v.peer(0)
        .save_truncate("yo.md", "version-2 (final)")
        .unwrap();

    // Both peers must end up with the final content, never empty.
    v.peer(0)
        .wait_for_content("yo.md", "version-2 (final)", T)
        .await
        .unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "version-2 (final)", T)
        .await
        .unwrap();

    // And it must STAY at v2 — guards against late propagation of the
    // intermediate empty state.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(v.peer(0).read("yo.md").unwrap(), "version-2 (final)");
    assert_eq!(v.rendezvous.read("yo.md").unwrap(), "version-2 (final)");

    v.shutdown().await;
}

#[tokio::test]
async fn rapid_truncate_saves_converge_on_final() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("doc.md", "0").unwrap();
    v.rendezvous
        .wait_for_content("doc.md", "0", T)
        .await
        .unwrap();

    // Hammer the file with five truncate-saves back to back.
    for i in 1..=5 {
        v.peer(0)
            .save_truncate("doc.md", &format!("revision-{}", i))
            .unwrap();
        // No sleep between saves — this is the worst case for the debouncer.
    }

    v.peer(0)
        .wait_for_content("doc.md", "revision-5", T)
        .await
        .unwrap();
    v.rendezvous
        .wait_for_content("doc.md", "revision-5", T)
        .await
        .unwrap();

    v.shutdown().await;
}

#[tokio::test]
async fn delete_propagates() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("ephemeral.md", "soon gone").unwrap();
    v.rendezvous
        .wait_for_content("ephemeral.md", "soon gone", T)
        .await
        .unwrap();

    v.peer(0).delete("ephemeral.md").unwrap();
    v.rendezvous
        .wait_for_missing("ephemeral.md", T)
        .await
        .unwrap();

    v.shutdown().await;
}

#[tokio::test]
async fn bidirectional_concurrent_writes_to_different_files() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0)
        .save_atomic("from-alice.md", "alice was here")
        .unwrap();
    v.rendezvous
        .save_atomic("from-server.md", "server says hi")
        .unwrap();

    v.rendezvous
        .wait_for_content("from-alice.md", "alice was here", T)
        .await
        .unwrap();
    v.peer(0)
        .wait_for_content("from-server.md", "server says hi", T)
        .await
        .unwrap();

    v.shutdown().await;
}

#[tokio::test]
async fn three_peer_fanout() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();
    let _ = v.add_peer("bob").await.unwrap();

    v.rendezvous.save_atomic("broadcast.md", "to all").unwrap();

    v.peer(0)
        .wait_for_content("broadcast.md", "to all", T)
        .await
        .unwrap();
    v.peer(1)
        .wait_for_content("broadcast.md", "to all", T)
        .await
        .unwrap();

    // Now alice writes; both server and bob should see it.
    v.peer(0).save_atomic("from-alice.md", "hi bob").unwrap();
    v.rendezvous
        .wait_for_content("from-alice.md", "hi bob", T)
        .await
        .unwrap();
    v.peer(1)
        .wait_for_content("from-alice.md", "hi bob", T)
        .await
        .unwrap();

    v.shutdown().await;
}

#[tokio::test]
async fn non_markdown_file_is_ignored() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("script.py", "print('nope')").unwrap();
    v.peer(0).save_atomic("note.md", "yes").unwrap();

    // The .md should sync; the .py should not exist on the rendezvous side.
    v.rendezvous
        .wait_for_content("note.md", "yes", T)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !v.rendezvous.exists("script.py"),
        "non-markdown file leaked across the sync"
    );

    v.shutdown().await;
}
