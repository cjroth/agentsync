//! Reconnect / connect-with-backoff e2e tests.
//!
//! Two scenarios:
//!  1. The rendezvous is killed and restarted on the same port. The peer
//!     should detect the disconnect, retry with backoff, and resume sync.
//!  2. The peer is started before the rendezvous exists. The peer should
//!     retry with backoff until the rendezvous comes up, then sync.

use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(15);

#[tokio::test]
async fn peer_reconnects_after_rendezvous_restart() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Confirm the initial connection works.
    v.peer(0).save_atomic("init.md", "before-restart").unwrap();
    v.rendezvous
        .wait_for_content("init.md", "before-restart", T)
        .await
        .unwrap();

    // Tear the rendezvous down. The peer's websocket will see EOF.
    v.kill_rendezvous().await.unwrap();

    // Give the peer a moment to notice and start retrying.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Restart on the same port — peer should reconnect via its backoff loop.
    v.restart_rendezvous().await.unwrap();

    // After reconnect, a write on the peer must propagate to the rendezvous.
    v.peer(0)
        .save_atomic("after.md", "after-restart")
        .unwrap();
    v.rendezvous
        .wait_for_content("after.md", "after-restart", T)
        .await
        .unwrap();

    // And changes that happened on the rendezvous after restart should also
    // travel back to the peer (full bidirectional sync, not a one-shot push).
    v.rendezvous
        .save_atomic("server-side.md", "from-server-after-restart")
        .unwrap();
    v.peer(0)
        .wait_for_content("server-side.md", "from-server-after-restart", T)
        .await
        .unwrap();

    v.shutdown().await;
}

#[tokio::test]
async fn peer_retries_initial_connect_until_rendezvous_starts() {
    // Vault state and a port are reserved, but no rendezvous process runs yet.
    let mut v = E2EVault::prepared_offline().await.unwrap();

    // Spawn a peer pointing at the not-yet-listening rendezvous. The peer
    // must keep retrying with backoff instead of bailing immediately.
    let _ = v.add_peer_without_waiting("alice").await.unwrap();

    // Let the peer make a few failing connect attempts.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Now bring the rendezvous up. The peer's next backoff attempt should
    // succeed.
    v.start_rendezvous().await.unwrap();

    // A write on the peer must eventually land on the rendezvous, proving
    // the peer reconnected (or rather, finally connected).
    v.peer(0)
        .save_atomic("late-connect.md", "i made it")
        .unwrap();
    v.rendezvous
        .wait_for_content("late-connect.md", "i made it", T)
        .await
        .unwrap();

    v.shutdown().await;
}
