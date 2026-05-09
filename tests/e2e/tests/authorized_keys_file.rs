//! `authorized_keys` is the SSH-style replacement for the old `peers.md`.
//! These tests pin the new filename, the SSH-style line format, and the
//! fact that the file syncs even though it has no extension.

use agentsync_core::Identity;
use agentsync_e2e::E2EVault;
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[tokio::test]
async fn init_seeds_authorized_keys_with_creator_pubkey() {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let printed_pub = stdout
        .lines()
        .find_map(|l| l.strip_prefix("identity_pub  = "))
        .unwrap()
        .trim()
        .to_string();

    // Run watch briefly to materialize the synced doc to disk.
    let mut child = tokio::process::Command::new(&binary)
        .arg("watch")
        .arg("--offline")
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = child.kill().await;

    let path = dir.path().join("authorized_keys");
    assert!(
        path.exists(),
        "authorized_keys was not materialized at {}",
        path.display()
    );
    // `peers.md` must NOT have been written.
    assert!(
        !dir.path().join("peers.md").exists(),
        "legacy peers.md was written; should be authorized_keys"
    );

    let body = std::fs::read_to_string(&path).unwrap();
    // SSH-style: at least one bare `ssh-ed25519 ...` line, no markdown bullets.
    let key_line = body
        .lines()
        .find(|l| l.trim_start().starts_with("ssh-ed25519 "))
        .unwrap_or_else(|| panic!("no ssh-ed25519 line in:\n{}", body));
    assert!(
        !key_line.contains("- `"),
        "authorized_keys is in markdown format, not SSH-style: {:?}",
        key_line
    );
    assert!(
        key_line.contains(printed_pub.split_whitespace().nth(1).unwrap()),
        "authorized_keys does not contain creator pubkey:\n{}",
        body
    );
}

#[tokio::test]
async fn appending_pubkey_to_authorized_keys_authorizes_peer() {
    // The harness writes the new filename. Two CLI peers, both authorized,
    // sync a file end-to-end. This makes sure the file actually syncs
    // despite having no extension.
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    v.peer(0)
        .save_atomic("hello.md", "via authorized_keys")
        .unwrap();
    v.rendezvous
        .wait_for_content("hello.md", "via authorized_keys", T)
        .await
        .unwrap();

    // The synced doc should contain a file named exactly "authorized_keys"
    // and it must materialize on disk despite having no file extension.
    let deadline = std::time::Instant::now() + T;
    while std::time::Instant::now() < deadline {
        if v.rendezvous.exists("authorized_keys") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(
        v.rendezvous.exists("authorized_keys"),
        "authorized_keys did not materialize on rendezvous"
    );

    v.shutdown().await;
}

#[tokio::test]
async fn watch_merges_authorized_keys_arg_after_restart() {
    // Regression: `agentsync watch --authorized-keys <pub>` (and the
    // equivalent `AGENTSYNC_AUTHORIZED_KEYS` env var, used for bootstrapping
    // hosted hubs from a Fly/Railway secret) must end up in the materialized
    // `authorized_keys` file alongside the creator key — *including* on a
    // restart where `authorized_keys` already exists on disk from a prior
    // boot. A previous boot ordering bug ran the merge before
    // `bind_directory`'s initial scan, which then ingested the on-disk
    // file and clobbered the merge — leaving the intended peer unauthorized
    // with no log line indicating anything was wrong.
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let init = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(init.status.success(), "init failed: {:?}", init);

    // First watch run to materialize authorized_keys to disk (creator only).
    // Mirrors the Fly scenario where the volume already has a file from a
    // previous boot before the operator adds the secret.
    let mut child = tokio::process::Command::new(&binary)
        .arg("watch")
        .arg("--offline")
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = child.kill().await;
    assert!(
        dir.path().join("authorized_keys").exists(),
        "first watch run did not materialize authorized_keys"
    );

    // A second identity that the operator now wants to authorize via the
    // env-var / flag bootstrap path.
    let bootstrap = Identity::generate();
    let bootstrap_pub = bootstrap.pubkey().to_ssh_string();

    let mut child = tokio::process::Command::new(&binary)
        .arg("watch")
        .arg("--offline")
        .arg("--authorized-keys")
        .arg(&bootstrap_pub)
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = child.kill().await;

    let body = std::fs::read_to_string(dir.path().join("authorized_keys")).unwrap();
    let parsed = agentsync_core::parse_authorized_keys(&body);
    assert!(
        parsed.iter().any(|p| p.pubkey == bootstrap.pubkey()),
        "bootstrap pubkey was not merged into authorized_keys after restart:\n{}",
        body
    );
    assert!(
        parsed.len() >= 2,
        "expected creator + bootstrap key, got {} entries:\n{}",
        parsed.len(),
        body
    );
}

#[tokio::test]
async fn deleting_authorized_keys_locally_does_not_propagate() {
    // Regression: `rm authorized_keys` (or `rm -rf` of the vault root) on a
    // running peer must not propagate the deletion to other peers — losing
    // the file empties the auth list and locks every peer out. The local
    // deletion is allowed to happen on disk; the doc keeps the file and the
    // materializer rewrites it on the next pass.
    let mut v = E2EVault::new().await.unwrap();
    let _ = v.add_peer("alice").await.unwrap();

    // Both peers should currently have authorized_keys synced.
    let deadline = std::time::Instant::now() + T;
    while std::time::Instant::now() < deadline {
        if v.peer(0).path().join("authorized_keys").exists()
            && v.rendezvous.exists("authorized_keys")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(v.peer(0).path().join("authorized_keys").exists());
    assert!(v.rendezvous.exists("authorized_keys"));

    // alice deletes authorized_keys locally — simulates the rm -rf-the-clone
    // case where the file is one of many being unlinked.
    std::fs::remove_file(v.peer(0).path().join("authorized_keys")).unwrap();

    // Give the watcher more than enough time to observe the unlink and
    // attempt to propagate it. If propagation happens, the rendezvous loses
    // the file.
    tokio::time::sleep(Duration::from_millis(800)).await;

    assert!(
        v.rendezvous.exists("authorized_keys"),
        "authorized_keys was deleted on the rendezvous after a peer unlinked \
         it locally — every peer is now locked out"
    );

    v.shutdown().await;
}

#[tokio::test]
async fn ssh_style_comments_are_ignored() {
    // The parser must accept SSH-style: bare key lines, '#' comments, blank lines.
    let id = Identity::generate();
    let pub_str = id.pubkey().to_ssh_string();
    let body = format!(
        "# This is a comment\n\n# Another comment\n{} alice\n# trailing comment\n",
        pub_str
    );
    let parsed = agentsync_core::parse_authorized_keys(&body);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].pubkey, id.pubkey());
    assert_eq!(parsed[0].label, "alice");
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
