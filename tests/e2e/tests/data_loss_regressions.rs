//! Regression tests for the user-reported data-loss races.
//!
//! Each test simulates a real-world editor-save scenario and asserts that no
//! peer ends up with the content silently emptied or overwritten.

use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

/// Bug #1: slow truncate-then-write outraces the fs-event debouncer.
///
/// Some editors leave the file empty for >150ms between `O_TRUNC` and the
/// actual `write()`. Without protection, the engine reads the empty
/// intermediate state, ingests it as a real change, and the empty propagates
/// across the sync — both peers' disks end up empty.
#[tokio::test]
async fn slow_truncate_save_does_not_propagate_empty() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Establish baseline: both peers have v1.
    v.peer(0).save_atomic("yo.md", "version-1").unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "version-1", T)
        .await
        .unwrap();

    // Save with a 350ms gap between truncate and write — well outside the
    // 150ms debounce window. This is the failure mode the user reported.
    v.peer(0)
        .save_truncate_with_gap("yo.md", "version-2 (final)", Duration::from_millis(350))
        .unwrap();

    // Both peers must converge on the final content. Crucially, content must
    // never be observable as empty on either side.
    v.peer(0)
        .wait_for_content("yo.md", "version-2 (final)", T)
        .await
        .unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "version-2 (final)", T)
        .await
        .unwrap();

    // And it must STAY at the final value — no late propagation of the
    // intermediate empty state.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let alice = v.peer(0).read("yo.md").unwrap();
    let server = v.rendezvous.read("yo.md").unwrap();
    assert_eq!(alice, "version-2 (final)", "alice flipped to: {:?}", alice);
    assert_eq!(
        server, "version-2 (final)",
        "server flipped to: {:?}",
        server
    );

    v.shutdown().await;
}

/// Stronger version of the above — a peer briefly observing empty content
/// during another peer's save would also be a regression. We poll fast enough
/// to catch a transient empty if one exists.
#[tokio::test]
async fn slow_truncate_never_empties_peer_disk() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("yo.md", "starting-content").unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "starting-content", T)
        .await
        .unwrap();

    // Background watcher: capture any time the rendezvous's view of yo.md
    // becomes empty during alice's save.
    let server_dir = v.rendezvous.path().to_path_buf();
    let watcher = tokio::spawn(async move {
        let path = server_dir.join("yo.md");
        let stop = std::time::Instant::now() + Duration::from_secs(3);
        let mut saw_empty = false;
        while std::time::Instant::now() < stop {
            if let Ok(s) = std::fs::read_to_string(&path) {
                if s.is_empty() {
                    saw_empty = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        saw_empty
    });

    // Slow truncate-write on alice.
    v.peer(0)
        .save_truncate_with_gap("yo.md", "ended-with-content", Duration::from_millis(350))
        .unwrap();

    let saw_empty = watcher.await.unwrap();
    assert!(
        !saw_empty,
        "rendezvous observed an empty yo.md during alice's slow save"
    );

    v.peer(0)
        .wait_for_content("yo.md", "ended-with-content", T)
        .await
        .unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "ended-with-content", T)
        .await
        .unwrap();

    v.shutdown().await;
}

/// Bug #2: incoming sync overwrites an uncommitted local edit.
///
/// While peer A's ingest is still debouncing the user's just-saved file, a
/// sync from peer B arrives and updates A's doc. The materializer (running on
/// a 100ms tick) sees the doc has new content and writes it to disk, clobbering
/// the user's saved file. Reproduced by saving the same file on both peers
/// at roughly the same moment, with one peer's save being slow enough to
/// cross the debounce.
#[tokio::test]
async fn local_edit_is_not_clobbered_by_concurrent_sync() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("yo.md", "shared-base").unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "shared-base", T)
        .await
        .unwrap();

    // Kick off a slow save on alice. While that's mid-flight, the server
    // also writes — its sync message will land at alice before alice's own
    // edit has been ingested.
    let alice_dir = v.peer(0).path().to_path_buf();
    let alice_save = std::thread::spawn(move || {
        let p = alice_dir.join("yo.md");
        std::fs::write(&p, b"").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        std::fs::write(&p, b"alice-edit").unwrap();
    });

    // Give alice's truncate a head start so its debounce starts ticking.
    std::thread::sleep(Duration::from_millis(50));
    v.rendezvous.save_atomic("yo.md", "server-edit").unwrap();

    alice_save.join().unwrap();

    // Allow time for sync convergence.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // After the dust settles, both peers must:
    //   1. Agree on the same content (CRDT convergence).
    //   2. Have non-empty content (neither edit was empty).
    let alice = v.peer(0).read("yo.md").unwrap();
    let server = v.rendezvous.read("yo.md").unwrap();
    assert_eq!(alice, server, "peers diverged: {:?} vs {:?}", alice, server);
    assert!(
        !alice.is_empty(),
        "content was lost — file ended up empty: {:?}",
        alice
    );
    // Whatever survived must contain at least one of the two edits' tokens.
    assert!(
        alice.contains("alice-edit") || alice.contains("server-edit"),
        "neither edit survived: {:?}",
        alice
    );

    v.shutdown().await;
}

/// Sharper version of bug #2: alice's locally-written edit must reach the
/// merged final content, not just be eventually replaced by the server's
/// version. This catches the materializer racing alice's debounced ingest
/// and clobbering her disk before her edit ever lands in the doc.
#[tokio::test]
async fn locally_written_edit_reaches_the_doc() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("yo.md", "BASE").unwrap();
    v.rendezvous
        .wait_for_content("yo.md", "BASE", T)
        .await
        .unwrap();

    // Server saves first; alice saves 30ms later. With 150ms debouncing,
    // server's sync message lands at alice's doc *before* alice's own
    // debounce fires. Without the materializer guard, alice's
    // materializer-on-tick happily writes the server's content to alice's
    // disk, marking her own freshly-saved file as 'just our own write' via
    // the suppression set, so her edit is silently dropped.
    let alice_dir = v.peer(0).path().to_path_buf();
    let server_dir = v.rendezvous.path().to_path_buf();
    let server_h = std::thread::spawn(move || {
        std::fs::write(server_dir.join("yo.md"), "SERVER_TOKEN_BBBB").unwrap();
    });
    std::thread::sleep(Duration::from_millis(30));
    let alice_h = std::thread::spawn(move || {
        std::fs::write(alice_dir.join("yo.md"), "ALICE_TOKEN_AAAA").unwrap();
    });
    server_h.join().unwrap();
    alice_h.join().unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    let alice = v.peer(0).read("yo.md").unwrap();
    let server = v.rendezvous.read("yo.md").unwrap();
    assert_eq!(alice, server, "peers diverged: {:?} / {:?}", alice, server);
    // Alice wrote ALICE_TOKEN to her own disk locally. That edit MUST make
    // it into the merged final state — anything else means her save was
    // silently overwritten.
    assert!(
        alice.contains("ALICE_TOKEN"),
        "alice's local edit was lost; converged content: {:?}",
        alice
    );

    v.shutdown().await;
}
