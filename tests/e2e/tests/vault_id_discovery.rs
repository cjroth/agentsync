//! Verifies vault_id discovery: a connecting peer can learn the hub's
//! vault_id from the handshake — the hub's HelloHub frame carries it.

use agentsync_core::{discover_vault_id, CreateOptions, Identity, Vault};
use agentsync_e2e::{authorize_in_process, E2EVault};
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::sleep;

#[tokio::test]
async fn discover_returns_servers_vault_id() {
    let server_dir = tempdir().unwrap();
    let storage = server_dir.path().join(".agentsync");
    let server_identity = Identity::generate();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: Some(server_identity.clone()),
        storage_path: storage,
    })
    .await
    .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("wss://{}", bound);

    // The probing client must be authorized — discovery does the full
    // handshake.
    let probe_identity = Identity::generate();
    authorize_in_process(&server, "probe", &probe_identity.pubkey()).await;

    let discovered = discover_vault_id(&url, &probe_identity).await.unwrap();
    assert_eq!(discovered, created.vault_id);
}

#[tokio::test]
async fn discover_rejects_unauthorized_peer() {
    let server_dir = tempdir().unwrap();
    let (mut server, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: None,
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("wss://{}", bound);

    let intruder = Identity::generate();
    let res = discover_vault_id(&url, &intruder).await;
    assert!(
        res.is_err(),
        "discovery with unauthorized identity must fail"
    );
}

/// `agentsync clone` against the harness rendezvous: the cloning device's
/// pubkey must be authorized first, then clone discovers vault_id during the
/// handshake and writes it into config.toml.
#[tokio::test]
async fn keyless_clone_via_subprocess() {
    let mut v = E2EVault::new().await.unwrap();
    let target = tempfile::TempDir::new().unwrap();
    let target_path = target.path().to_path_buf();

    let binary = std::env::var("AGENTSYNC_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            manifest_dir
                .ancestors()
                .nth(2)
                .unwrap()
                .join("target")
                .join("debug")
                .join("agentsync")
        });

    // Pre-create a fresh identity for the cloning device and authorize it on
    // the hub.
    let clone_identity = Identity::generate();
    let id_dir = tempfile::TempDir::new().unwrap();
    let id_path = id_dir.path().join("id");
    clone_identity.save_to_file(&id_path).unwrap();
    v.authorize_peer("cloner", &clone_identity.pubkey())
        .await
        .unwrap();

    let hub_pubkey = v.rendezvous.identity.pubkey().to_ssh_string();
    let mut child = tokio::process::Command::new(&binary)
        .arg("clone")
        .arg(&target_path)
        .arg("--rendezvous")
        .arg(&v.rendezvous_url)
        .arg("--accept-hub-key")
        .arg(&hub_pubkey)
        .arg("--identity")
        .arg(&id_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let _ = child.kill().await;

    let cfg_path = target_path.join(".agentsync").join("config.toml");
    assert!(cfg_path.exists(), "clone did not write config.toml");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        cfg.contains(&format!("id = \"{}\"", v.vault_id)),
        "config.toml does not carry the discovered vault_id; got:\n{}",
        cfg
    );

    drop(v);
}
