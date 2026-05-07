//! Relative-time formats accepted by `agentsync restore-at`.
//!
//! The user types `5m`, `2h`, `1d`, `1w` to mean "5 minutes ago", etc.
//! Plain integers continue to be epoch milliseconds. The CLI prints back
//! the resolved millisecond timestamp so we can verify the math without
//! coupling to any in-memory state.

use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn restore_at_accepts_minutes_suffix() {
    let dir = init_vault().await;
    let binary = locate_binary();
    let now_ms = now_ms();

    let out = tokio::process::Command::new(&binary)
        .arg("restore-at")
        .arg("5m")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "restore-at 5m failed: {:?}", out);

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let resolved = extract_resolved_ms(&stdout)
        .unwrap_or_else(|| panic!("could not parse resolved ms from: {}", stdout));
    let five_min_ms = 5 * 60 * 1000;
    let expected = now_ms - five_min_ms;
    let drift = (resolved - expected).abs();
    assert!(
        drift < 5_000,
        "5m resolved to {} ms, expected ~{} ms (drift {})",
        resolved,
        expected,
        drift
    );
}

#[tokio::test]
async fn restore_at_accepts_hours_days_weeks() {
    let dir = init_vault().await;
    let binary = locate_binary();
    let now = now_ms();

    for (input, ms_offset) in [
        ("2h", 2 * 60 * 60 * 1000),
        ("1d", 24 * 60 * 60 * 1000),
        ("1w", 7 * 24 * 60 * 60 * 1000),
    ] {
        let out = tokio::process::Command::new(&binary)
            .arg("restore-at")
            .arg(input)
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        assert!(out.status.success(), "restore-at {} failed: {:?}", input, out);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let resolved = extract_resolved_ms(&stdout).unwrap();
        let expected = now - ms_offset;
        let drift = (resolved - expected).abs();
        assert!(
            drift < 10_000,
            "{} resolved to {}, expected ~{}, drift {}",
            input,
            resolved,
            expected,
            drift
        );
    }
}

#[tokio::test]
async fn restore_at_still_accepts_epoch_ms() {
    let dir = init_vault().await;
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("restore-at")
        .arg("1700000000000")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "restore-at with epoch ms failed: {:?}",
        out
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let resolved = extract_resolved_ms(&stdout).unwrap();
    assert_eq!(resolved, 1_700_000_000_000);
}

#[tokio::test]
async fn restore_at_rejects_garbage() {
    let dir = init_vault().await;
    let binary = locate_binary();

    let out = tokio::process::Command::new(&binary)
        .arg("restore-at")
        .arg("five-minutes")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success(), "garbage input must fail");
}

async fn init_vault() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let binary = locate_binary();
    let out = tokio::process::Command::new(&binary)
        .arg("init")
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "init failed: {:?}", out);
    dir
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Find the resolved millisecond timestamp the CLI echoes back. Looks for
/// a line shaped `restored to <input> (<ms>)`.
fn extract_resolved_ms(out: &str) -> Option<i64> {
    for line in out.lines() {
        let trim = line.trim();
        if let Some(rest) = trim.strip_prefix("restored to ") {
            let ms = rest.rsplit_once('(')?.1.trim_end_matches(')');
            return ms.parse().ok();
        }
    }
    None
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
