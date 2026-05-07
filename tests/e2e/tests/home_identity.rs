//! Default identity now lives at `~/.agentsync/id_ed25519` so a single
//! ed25519 keypair is shared across all vaults a user owns (matching SSH's
//! `~/.ssh/id_ed25519` convention). The `--identity` flag overrides.

use std::time::Duration;

#[tokio::test]
async fn init_creates_identity_in_home_directory() {
    let home = tempfile::TempDir::new().unwrap();
    let vault = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(vault.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);

    let home_id = home.path().join(".agentsync").join("id_ed25519");
    assert!(
        home_id.exists(),
        "default identity not created at {}",
        home_id.display()
    );
    assert!(
        home_id.with_extension("pub").exists(),
        "id_ed25519.pub sidecar missing"
    );

    let per_vault = vault.path().join(".agentsync").join("identity");
    assert!(
        !per_vault.exists(),
        "per-vault identity should NOT be created when home default is used"
    );
}

#[tokio::test]
async fn second_init_reuses_existing_home_identity() {
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let vault_a = tempfile::TempDir::new().unwrap();
    let out_a = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(vault_a.path())
        .output()
        .await
        .unwrap();
    assert!(out_a.status.success());
    let pub_a = extract_pubkey(&out_a.stdout);

    let vault_b = tempfile::TempDir::new().unwrap();
    let out_b = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(vault_b.path())
        .output()
        .await
        .unwrap();
    assert!(out_b.status.success());
    let pub_b = extract_pubkey(&out_b.stdout);

    assert_eq!(
        pub_a, pub_b,
        "second init should reuse the home-dir identity, not generate a new one"
    );
}

#[tokio::test]
async fn identity_flag_overrides_home_default() {
    let home = tempfile::TempDir::new().unwrap();
    let custom = tempfile::TempDir::new().unwrap();
    let custom_path = custom.path().join("my-key");
    let vault = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .arg("--identity")
        .arg(&custom_path)
        .env("HOME", home.path())
        .current_dir(vault.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);

    assert!(
        custom_path.exists(),
        "custom identity not created at {}",
        custom_path.display()
    );
    assert!(
        !home.path().join(".agentsync").join("id_ed25519").exists(),
        "home identity should NOT exist when --identity is given"
    );
}

#[tokio::test]
async fn key_show_reads_home_identity_after_init() {
    let home = tempfile::TempDir::new().unwrap();
    let vault = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let init = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(vault.path())
        .output()
        .await
        .unwrap();
    assert!(init.status.success());
    let init_pub = extract_pubkey(&init.stdout);

    let show = tokio::process::Command::new(&binary)
        .arg("key")
        .arg("show")
        .env("HOME", home.path())
        .current_dir(vault.path())
        .output()
        .await
        .unwrap();
    assert!(show.status.success(), "key show failed: {:?}", show);
    let show_pub = String::from_utf8_lossy(&show.stdout)
        .lines()
        .next()
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(show_pub, init_pub);
}

#[tokio::test]
async fn key_generate_creates_home_identity_when_no_path_given() {
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    // No init — just key generate. Should land at ~/.agentsync/id_ed25519.
    // (key generate also takes a path arg defaulting to "."; since no
    // .agentsync/config.toml exists, it should still default to home.)
    let work = tempfile::TempDir::new().unwrap();
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&binary)
            .arg("key")
            .arg("generate")
            .env("HOME", home.path())
            .current_dir(work.path())
            .output(),
    )
    .await
    .expect("key generate timed out")
    .unwrap();
    assert!(out.status.success(), "key generate failed: {:?}", out);

    let home_id = home.path().join(".agentsync").join("id_ed25519");
    assert!(
        home_id.exists(),
        "key generate did not write home identity at {}",
        home_id.display()
    );
}

fn extract_pubkey(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    s.lines()
        .find_map(|l| l.strip_prefix("identity_pub  = "))
        .unwrap_or_else(|| panic!("no identity_pub line in:\n{}", s))
        .trim()
        .to_string()
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
