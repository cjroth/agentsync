//! Phase 4 — TOFU hub trust via `[vault] hub_pubkey` in config.toml.
//!
//! Pins the user-visible behavior: a fresh clone with `--accept-hub-key`
//! pre-pins the hub identity in config.toml; subsequent watches are silent;
//! a swapped hub identity makes the next connect fail with a mismatch
//! error.

use agentsync_core::{BindOptions, CreateOptions, Identity, OpenOptions, Vault};
use agentsync_e2e::{authorize_in_process, E2EVault};
use std::time::Duration;
use tempfile::tempdir;

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

/// `--accept-hub-key` pre-pins the hub identity. After clone, config.toml
/// carries the pinned key under `[vault] hub_pubkey`.
#[tokio::test]
async fn accept_hub_key_pins_in_config() {
    let mut v = E2EVault::new().await.unwrap();
    let target = tempfile::TempDir::new().unwrap();
    let target_path = target.path().to_path_buf();
    let binary = locate_binary();

    let id = Identity::generate();
    let id_dir = tempfile::TempDir::new().unwrap();
    let id_path = id_dir.path().join("id");
    id.save_to_file(&id_path).unwrap();
    v.authorize_peer("cloner", &id.pubkey()).await.unwrap();

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
        cfg.contains(&format!("hub_pubkey = \"{}\"", hub_pubkey)),
        "hub_pubkey not pinned in config.toml; got:\n{}",
        cfg
    );
}

/// A pinned hub_pubkey makes a fresh in-process connect succeed silently
/// when it matches, and fail with an Auth error when it doesn't.
#[tokio::test]
async fn pinned_mismatch_is_rejected_in_process() {
    let server_dir = tempdir().unwrap();
    let server_identity = Identity::generate();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: Some(server_identity.clone()),
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let _ = server
        .bind_directory(server_dir.path(), BindOptions::default())
        .await
        .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("wss://{}", bound);

    let client_identity = Identity::generate();
    authorize_in_process(&server, "client", &client_identity.pubkey()).await;

    // Matching pin: connect succeeds.
    let dir_ok = tempdir().unwrap();
    let mut ok_client = Vault::open(OpenOptions {
        rendezvous_url: Some(url.clone()),
        vault_id: created.vault_id.clone(),
        identity: client_identity.clone(),
        storage_path: dir_ok.path().join(".agentsync"),
        hub_pubkey: Some(server_identity.pubkey()),
        name: None,
    })
    .await
    .unwrap();
    ok_client.connect().await.expect("matching pin should succeed");
    ok_client.disconnect().await;

    // Mismatched pin: connect fails.
    let dir_bad = tempdir().unwrap();
    let other_identity = Identity::generate();
    let mut bad_client = Vault::open(OpenOptions {
        rendezvous_url: Some(url),
        vault_id: created.vault_id.clone(),
        identity: client_identity,
        storage_path: dir_bad.path().join(".agentsync"),
        hub_pubkey: Some(other_identity.pubkey()),
        name: None,
    })
    .await
    .unwrap();
    let err = bad_client
        .connect()
        .await
        .expect_err("mismatched pin should reject");
    let msg = err.to_string();
    assert!(
        msg.contains("hub identity mismatch"),
        "expected 'hub identity mismatch' error, got: {}",
        msg
    );
}

/// `agentsync hub trust/forget/show` round-trips the pinned value.
#[tokio::test]
async fn hub_subcommands_round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    // init the vault first (creates config.toml).
    let init = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(init.status.success(), "init failed: {:?}", init);

    // hub show on a fresh vault: no pin.
    let show = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("show")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(show.status.success(), "hub show failed: {:?}", show);
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(
        stdout.contains("(no hub_pubkey pinned)"),
        "unexpected hub show output: {}",
        stdout
    );

    // Pin a key.
    let id = Identity::generate();
    let pk_str = id.pubkey().to_ssh_string();
    let trust = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("trust")
        .arg(&pk_str)
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(trust.status.success(), "hub trust failed: {:?}", trust);

    // Now hub show prints it.
    let show = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("show")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(
        stdout.contains(&pk_str),
        "hub show did not return pinned key; got: {}",
        stdout
    );

    // Forget clears it.
    let forget = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("forget")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(forget.status.success(), "hub forget failed: {:?}", forget);

    let show = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("show")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(
        stdout.contains("(no hub_pubkey pinned)"),
        "forget didn't clear; got: {}",
        stdout
    );
}

/// Hub trust refuses to persist garbage. Catches typos before they end up
/// silently breaking later connects.
#[tokio::test]
async fn hub_trust_validates_pubkey() {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();
    let _ = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let bad = tokio::process::Command::new(&binary)
        .arg("hub")
        .arg("trust")
        .arg("not-a-real-pubkey")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(
        !bad.status.success(),
        "hub trust accepted invalid input: {:?}",
        bad
    );
}
