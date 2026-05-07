//! `agentsync completions <shell>` emits a shell completion script for the
//! requested shell. The script must mention every top-level subcommand so
//! tab-completion at the prompt yields the right suggestions.

#[tokio::test]
async fn completions_bash_includes_subcommands() {
    let out = tokio::process::Command::new(locate_binary())
        .arg("completions")
        .arg("bash")
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "completions bash failed: {:?}", out);
    let body = String::from_utf8_lossy(&out.stdout);
    for sub in [
        "init",
        "watch",
        "clone",
        "status",
        "push",
        "pull",
        "restore-at",
        "snapshot",
        "diff",
        "compact",
        "key",
        "hub",
    ] {
        assert!(
            body.contains(sub),
            "bash completion script missing subcommand {:?}",
            sub
        );
    }
}

#[tokio::test]
async fn completions_zsh_emits_compdef_block() {
    let out = tokio::process::Command::new(locate_binary())
        .arg("completions")
        .arg("zsh")
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "completions zsh failed: {:?}", out);
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(
        body.contains("#compdef agentsync"),
        "zsh completion missing `#compdef agentsync` header"
    );
}

#[tokio::test]
async fn completions_fish_works() {
    let out = tokio::process::Command::new(locate_binary())
        .arg("completions")
        .arg("fish")
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "completions fish failed: {:?}", out);
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("complete -c agentsync"));
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
