//! Phase 1 — per-peer ed25519 identities and `authorized_keys` authorization.
//!
//! These tests pin down the user-visible behavior of identity-based auth:
//! the CLI's `key` subcommand surface, the hub's gating on `authorized_keys`, and
//! the listener's reaction to a peer being removed mid-session.

use agentsync_core::{Identity, Pubkey};
use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

/// `agentsync init` writes a fresh identity, prints its pubkey, and seeds
/// authorized_keys with the creator's pubkey so the very first listener accepts
/// connections from itself / its in-process peers.
#[tokio::test]
async fn init_creates_identity_and_seeds_authorized_keys() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("identity_pub  = ssh-ed25519 "),
        "init did not print identity_pub: {}",
        stdout
    );

    let identity_path = home.path().join(".agentsync").join("id_ed25519");
    assert!(
        identity_path.exists(),
        "~/.agentsync/id_ed25519 not created at {}",
        identity_path.display()
    );
    // The seed file must round-trip into a valid Identity.
    let id = Identity::load_from_file(&identity_path).unwrap();
    let printed_pub = stdout
        .lines()
        .find_map(|l| l.strip_prefix("identity_pub  = "))
        .unwrap()
        .trim();
    assert_eq!(printed_pub, id.pubkey().to_ssh_string());

    // The doc seeded authorized_keys with this pubkey.
    // Materialize authorized_keys to disk by running watch briefly so we can read it.
    let mut child = tokio::process::Command::new(&binary)
        .arg("watch")
        .arg("--offline")
        .env("HOME", home.path())
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = child.kill().await;

    let authorized_keys_path = dir.path().join("authorized_keys");
    assert!(
        authorized_keys_path.exists(),
        "authorized_keys was not materialized to disk by watch"
    );
    let body = std::fs::read_to_string(&authorized_keys_path).unwrap();
    assert!(
        body.contains(printed_pub),
        "authorized_keys does not contain creator pubkey:\n{}",
        body
    );
}

/// `agentsync key show` prints the local pubkey in `ssh-ed25519 ...` form.
#[tokio::test]
async fn key_show_prints_pubkey() {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let init = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(init.status.success());

    let show = tokio::process::Command::new(&binary)
        .arg("key")
        .arg("show")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(show.status.success(), "key show failed: {:?}", show);
    let printed = String::from_utf8(show.stdout).unwrap();
    let line = printed.lines().next().unwrap().trim();
    assert!(
        line.starts_with("ssh-ed25519 "),
        "key show output not ssh-ed25519: {:?}",
        line
    );
    // And it must parse back to a Pubkey.
    let _ = Pubkey::from_ssh_string(line).unwrap();
}

/// Two CLI peers, both authorized in authorized_keys, sync end to end. Drives the
/// happy path through the new four-message handshake.
#[tokio::test]
async fn authorized_peers_sync_end_to_end() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0).save_atomic("hello.md", "via identity").unwrap();
    v.rendezvous
        .wait_for_content("hello.md", "via identity", T)
        .await
        .unwrap();

    v.shutdown().await;
}

/// A peer whose pubkey is not in authorized_keys must not sync. The subprocess may
/// stay alive (the reconnect supervisor keeps trying), but writes from the
/// hub never reach the unauthorized peer's disk.
#[tokio::test]
async fn unauthorized_peer_does_not_sync() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_unauthorized_peer("intruder").await.unwrap();

    // Hub writes a file. An authorized peer would receive it within
    // milliseconds. Wait a generous window and assert the intruder's disk
    // remains empty.
    v.rendezvous
        .save_atomic("secret.md", "should-not-leak")
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let intruder = &v.peers[0];
    assert!(
        !intruder.exists("secret.md"),
        "unauthorized peer received hub content — auth gate broken"
    );

    v.shutdown().await;
}

/// Removing a peer's pubkey from authorized_keys disconnects them within a short
/// window. Specifically: writes from the now-deauthorized peer must stop
/// propagating to the hub.
#[tokio::test]
async fn deauthorize_disconnects_peer() {
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Sanity: alice can sync.
    v.peer(0).save_atomic("first.md", "before").unwrap();
    v.rendezvous
        .wait_for_content("first.md", "before", T)
        .await
        .unwrap();

    // Yank alice from authorized_keys.
    let alice_pk = v.peer(0).pubkey();
    v.deauthorize_peer(&alice_pk).await.unwrap();

    // The hub should drop alice's connection. Wait up to 5s for the disconnect
    // to register, then assert that a fresh write from alice does NOT show up
    // on the hub.
    tokio::time::sleep(Duration::from_secs(2)).await;
    v.peer(0)
        .save_atomic("second.md", "should not propagate")
        .unwrap();

    // Give it generous propagation time. With the hub having dropped the
    // connection, alice's writes won't reach it. Without auth enforcement,
    // they would.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !v.rendezvous.exists("second.md"),
        "deauthorized peer's write still reached the hub — auth enforcement broken"
    );

    v.shutdown().await;
}

/// `agentsync clone` uses the local identity to authenticate. With the
/// pubkey pre-staged and authorized on the hub, clone discovers vault_id
/// and writes config.toml without any shared-secret env vars in scope.
#[tokio::test]
async fn clone_uses_local_identity() {
    let mut v = E2EVault::new().await.unwrap();
    let target = tempfile::TempDir::new().unwrap();
    let target_path = target.path().to_path_buf();
    let binary = locate_binary();

    let id = Identity::generate();
    let id_dir = tempfile::TempDir::new().unwrap();
    let id_path = id_dir.path().join("id");
    id.save_to_file(&id_path).unwrap();
    v.authorize_peer("cloner", &id.pubkey()).await.unwrap();

    // Pin the hub up front via --accept-hub-key so the subprocess doesn't
    // block on the interactive trust prompt. Pass --identity so the clone
    // uses our pre-authorized key instead of generating a fresh one.
    let hub_pubkey = v.rendezvous.identity.pubkey().to_ssh_string();
    let mut child = tokio::process::Command::new(&binary)
        .arg("clone")
        .arg(&v.rendezvous_url)
        .arg(&target_path)
        .arg("--accept-hub-key")
        .arg(&hub_pubkey)
        .arg("--identity")
        .arg(&id_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = child.kill().await;

    let cfg = std::fs::read_to_string(target_path.join(".agentsync").join("config.toml"))
        .unwrap();
    assert!(
        cfg.contains(&format!("id = \"{}\"", v.vault_id)),
        "clone did not record vault_id; config.toml:\n{}",
        cfg
    );
}

fn locate_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("AGENTSYNC_BIN") {
        return std::path::PathBuf::from(p);
    }
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .unwrap()
        .join("target")
        .join("debug")
        .join("agentsync")
}
