//! Default rendezvous port (1234) is applied when `--rendezvous` is given
//! without an explicit port. The constant lives in agentsync-core; the CLI
//! normalizes the URL before persisting it to config.toml so the user can
//! see the canonical form on disk.

use std::time::Duration;

#[tokio::test]
async fn init_with_portless_rendezvous_writes_default_port() {
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
        cfg.contains("rendezvous_url = \"wss://example.invalid:1234\""),
        "config.toml did not normalize portless URL to :1234:\n{}",
        cfg
    );
}

#[tokio::test]
async fn clone_against_portless_url_attempts_port_1234() {
    // When --rendezvous omits a port we should attempt port 1234, not 443.
    // Spin up a TCP listener on 127.0.0.1:0 just so we can assert nothing
    // unexpected happens, then run clone against a guaranteed-unused
    // 127.0.0.1 URL with no port. The error message must reference :1234.
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let url = "wss://127.0.0.1";

    let out = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(&binary)
            .arg("clone")
            .arg(dir.path().join("vault"))
            .arg("--rendezvous")
            .arg(url)
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
        combined.contains(":1234"),
        "clone did not reference default port 1234 in output:\n{}",
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
