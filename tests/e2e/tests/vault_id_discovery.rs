//! Verifies option-1: a client can connect with only a rendezvous URL and a
//! key, and the server returns its vault_id during the handshake.

use agentsync_core::{discover_vault_id, encode_key, generate_vault_key, CreateOptions, Vault};
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::sleep;

#[tokio::test]
async fn discover_returns_servers_vault_id() {
    // Set up an in-process listener with a known vault_id.
    let server_dir = tempdir().unwrap();
    let storage = server_dir.path().join(".agentsync");
    let key = generate_vault_key();
    let (mut server, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        vault_key: Some(key),
        storage_path: storage,
    })
    .await
    .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("ws://{}", bound);

    // Discovery dance — no vault_id sent.
    let discovered = discover_vault_id(&url, key).await.unwrap();
    assert_eq!(discovered, created.vault_id);
}

#[tokio::test]
async fn discover_rejects_wrong_key() {
    let server_dir = tempdir().unwrap();
    let key = generate_vault_key();
    let (mut server, _) = Vault::create(CreateOptions {
        rendezvous_url: None,
        vault_key: Some(key),
        storage_path: server_dir.path().join(".agentsync"),
    })
    .await
    .unwrap();
    let bound = server.listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let url = format!("ws://{}", bound);

    let bad_key = [0u8; 32];
    let res = discover_vault_id(&url, bad_key).await;
    assert!(res.is_err(), "discovery with wrong key must fail");
}

#[tokio::test]
async fn keyless_clone_via_subprocess() {
    use agentsync_e2e::E2EVault;

    // We re-use the harness (which already starts a real `--listen` peer) and
    // then invoke `agentsync clone` against it WITHOUT a vault_id arg.
    let v = E2EVault::new().await.unwrap();
    let target = tempfile::TempDir::new().unwrap();
    let target_path = target.path().to_path_buf();

    let binary = std::env::var("AGENTSYNC_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            // Same lookup pattern the harness uses internally.
            let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            manifest_dir
                .ancestors()
                .nth(2)
                .unwrap()
                .join("target")
                .join("debug")
                .join("agentsync")
        });

    // `clone <local-path> --rendezvous URL` with $AGENTSYNC_KEY in env.
    let mut child = tokio::process::Command::new(&binary)
        .arg("clone")
        .arg(&target_path)
        .arg("--rendezvous")
        .arg(&v.rendezvous_url)
        .env("AGENTSYNC_KEY", &v.vault_key_b64)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Wait for clone to settle, then kill it.
    sleep(Duration::from_secs(2)).await;
    let _ = child.kill().await;

    // The cloned dir should now have a config.toml whose vault_id matches the
    // server's (no UUID was passed on the command line).
    let cfg_path = target_path.join(".agentsync").join("config.toml");
    assert!(cfg_path.exists(), "clone did not write config.toml");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        cfg.contains(&format!("id = \"{}\"", v.vault_id)),
        "config.toml does not carry the discovered vault_id; got:\n{}",
        cfg
    );

    drop(v);
    let _ = encode_key; // keep the import used
}
