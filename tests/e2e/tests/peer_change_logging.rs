//! When a pubkey is added to or removed from `authorized_keys`, the hub
//! logs a clear "peer added" / "peer removed" line at info level. Removing
//! a peer must also avoid the bogus "unknown peer N" warning that used to
//! fire when a sync message arrived for a just-disconnected peer.

use agentsync_core::Identity;
use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(8);

#[tokio::test]
async fn adding_pubkey_logs_peer_added() {
    let mut v = E2EVault::new().await.unwrap();

    let id = Identity::generate();
    v.authorize_peer("alice", &id.pubkey()).await.unwrap();

    let line = v
        .rendezvous
        .wait_for_stderr(|l| l.contains("peer added") && l.contains("alice"), T)
        .await
        .unwrap_or_else(|| {
            panic!(
                "hub did not log `peer added` for alice. Captured stderr:\n{}",
                v.rendezvous.stderr_dump()
            )
        });
    assert!(line.contains("alice"));

    v.shutdown().await;
}

#[tokio::test]
async fn removing_pubkey_logs_peer_removed_and_no_unknown_peer_noise() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Confirm sync works first.
    v.peer(0).save_atomic("hello.md", "hi").unwrap();
    v.rendezvous
        .wait_for_content("hello.md", "hi", T)
        .await
        .unwrap();

    // Yank alice's key.
    let alice_pk = v.peer(0).pubkey();
    v.deauthorize_peer(&alice_pk).await.unwrap();

    let removed = v
        .rendezvous
        .wait_for_stderr(|l| l.contains("peer removed"), T)
        .await
        .unwrap_or_else(|| {
            panic!(
                "hub did not log `peer removed`. Captured stderr:\n{}",
                v.rendezvous.stderr_dump()
            )
        });
    assert!(
        removed.contains("alice") || removed.contains(&alice_pk.fingerprint_sha256()),
        "peer-removed line missing identifier:\n{}",
        removed
    );

    // Wait long enough for any post-disconnect race to settle, then assert
    // we did NOT log `unknown peer N`.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let dump = v.rendezvous.stderr_dump();
    assert!(
        !dump.contains("unknown peer"),
        "hub logged confusing `unknown peer` after deauthorize:\n{}",
        dump
    );

    v.shutdown().await;
}
