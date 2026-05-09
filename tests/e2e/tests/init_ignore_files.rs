//! `agentsync init` adds `.agentsync/` to `.gitignore` and `.agentsignore`
//! and seeds a `.syncignore` (gitignore-syntax) by default. Existing files
//! are appended to (no duplicate lines); `.syncignore` is left alone if it
//! already exists. `--no-ignore-files` opts out of all three.

use std::time::Duration;

#[tokio::test]
async fn init_creates_ignore_files_by_default() {
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

    let git = dir.path().join(".gitignore");
    let agent = dir.path().join(".agentsignore");
    let sync = dir.path().join(".syncignore");
    assert!(git.exists(), ".gitignore not created");
    assert!(agent.exists(), ".agentsignore not created");
    assert!(sync.exists(), ".syncignore not created");

    let g = std::fs::read_to_string(&git).unwrap();
    let a = std::fs::read_to_string(&agent).unwrap();
    let s = std::fs::read_to_string(&sync).unwrap();
    assert!(
        g.lines().any(|l| l.trim() == ".agentsync/"),
        ".gitignore missing `.agentsync/` line:\n{}",
        g
    );
    assert!(
        a.lines().any(|l| l.trim() == ".agentsync/"),
        ".agentsignore missing `.agentsync/` line:\n{}",
        a
    );
    // `.syncignore` body is opinionated — assert the defaults we care about.
    assert!(
        s.lines().any(|l| l.trim() == "node_modules/"),
        ".syncignore missing `node_modules/`:\n{}",
        s
    );
    assert!(
        s.lines().any(|l| l.trim() == ".DS_Store"),
        ".syncignore missing `.DS_Store`:\n{}",
        s
    );
}

#[tokio::test]
async fn init_leaves_existing_syncignore_alone() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let custom = "# my own rules\nbuild/\n!build/keep.md\n";
    std::fs::write(dir.path().join(".syncignore"), custom).unwrap();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success());

    let s = std::fs::read_to_string(dir.path().join(".syncignore")).unwrap();
    assert_eq!(s, custom, "init should not modify an existing .syncignore");
}

#[tokio::test]
async fn init_appends_to_existing_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    std::fs::write(dir.path().join(".gitignore"), "node_modules/\ntarget/\n").unwrap();
    std::fs::write(dir.path().join(".agentsignore"), "scratch/\n").unwrap();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success());

    let g = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(
        g.contains("node_modules/"),
        "preserved existing .gitignore content"
    );
    assert!(g.contains("target/"));
    assert!(g.lines().any(|l| l.trim() == ".agentsync/"));

    let a = std::fs::read_to_string(dir.path().join(".agentsignore")).unwrap();
    assert!(a.contains("scratch/"));
    assert!(a.lines().any(|l| l.trim() == ".agentsync/"));
}

#[tokio::test]
async fn init_does_not_duplicate_existing_entry() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    std::fs::write(dir.path().join(".gitignore"), ".agentsync/\nother/\n").unwrap();

    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .env("HOME", home.path())
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success());

    let g = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    let count = g.lines().filter(|l| l.trim() == ".agentsync/").count();
    assert_eq!(count, 1, "duplicated .agentsync/ in .gitignore:\n{}", g);
}

#[tokio::test]
async fn no_ignore_files_flag_skips_creation() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();

    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&binary)
            .arg("init")
            .arg("--no-ignore-files")
            .env("HOME", home.path())
            .current_dir(dir.path())
            .output(),
    )
    .await
    .expect("init timed out")
    .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);

    assert!(!dir.path().join(".gitignore").exists());
    assert!(!dir.path().join(".agentsignore").exists());
    assert!(!dir.path().join(".syncignore").exists());
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
