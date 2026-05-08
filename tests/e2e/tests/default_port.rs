//! Port-less rendezvous URLs are persisted as-is — the WebSocket client
//! uses the scheme default (443 for wss, 80 for ws) when no port is given.
//! This keeps `wss://my-hub.fly.dev` working through a reverse proxy that
//! terminates TLS on 443. Self-hosted hubs running on `[`DEFAULT_PORT`]`
//! (1234, the unprivileged `--listen` default) must include `:1234` in
//! their URL.

use std::time::Duration;

#[tokio::test]
async fn init_with_portless_rendezvous_persists_url_as_is() {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .arg("--rendezvous")
        .arg("wss://example.invalid")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);

    let cfg = std::fs::read_to_string(dir.path().join(".agentsync").join("config.toml"))
        .unwrap();
    assert!(
        cfg.contains("rendezvous_url = \"wss://example.invalid\""),
        "config.toml mutated portless URL:\n{}",
        cfg
    );
    assert!(
        !cfg.contains(":1234"),
        "config.toml unexpectedly added :1234 to portless URL:\n{}",
        cfg
    );
}

#[tokio::test]
async fn clone_against_portless_url_does_not_use_1234() {
    // When --rendezvous omits a port we should attempt the scheme default
    // (443 for wss) — not 1234. This guards the Fly.io / reverse-proxy
    // deployment path. We can't easily observe the connect port from the
    // OS error string, so the regression assertion is negative: nothing in
    // the output should mention :1234.
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let url = "wss://127.0.0.1";

    let out = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(&binary)
            .arg("clone")
            .arg(url)
            .arg(dir.path().join("vault"))
            .arg("--accept-hub-key")
            .arg("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .output(),
    )
    .await
    .expect("clone command did not exit in time")
    .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let combined = format!("{}\n{}", stdout, stderr);

    assert!(
        !combined.contains(":1234"),
        "clone unexpectedly referenced port 1234 for a portless URL:\n{}",
        combined
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
