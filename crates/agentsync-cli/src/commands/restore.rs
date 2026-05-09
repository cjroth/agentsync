use crate::cli::RestoreAtArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::{Result, bail};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn run_restore_at(cwd: PathBuf, args: RestoreAtArgs) -> Result<()> {
    let path = cwd.canonicalize().unwrap_or(cwd);
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let identity = config::resolve_identity(&path, &cfg)?;

    let target_ms = parse_timestamp(&args.timestamp, now_ms())?;
    let opts = OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        identity,
        storage_path: path.join(".agentsync"),
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
        name: cfg.vault.name.clone(),
    };
    let mut vault = Vault::open(opts).await?;
    let binding = vault
        .bind_directory(&path, cfg.sync.to_bind_options())
        .await?;
    vault.restore_to_time(target_ms).await?;
    vault.materialize(&binding).await?;
    vault.flush().await?;
    println!("restored to {} ({})", args.timestamp, target_ms);
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Accepts:
///   - plain integer → epoch milliseconds
///   - relative offset `<n><unit>` where unit is s/m/h/d/w (always backwards
///     from `now_ms`)
fn parse_timestamp(s: &str, now_ms: i64) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty timestamp");
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    if let Some(offset_ms) = parse_relative_ms(s) {
        return Ok(now_ms - offset_ms);
    }
    bail!(
        "unrecognised timestamp {:?}: expected epoch ms (e.g. 1700000000000) \
         or a relative offset like 5m / 2h / 1d / 1w",
        s
    )
}

/// Parse a relative-time string like `5m`, `2h`, `1d`, `1w` into a
/// millisecond offset from now. Returns `None` if the format doesn't match.
fn parse_relative_ms(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let (num, unit) = bytes.split_at(bytes.len() - 1);
    let num: i64 = std::str::from_utf8(num).ok()?.parse().ok()?;
    let unit_ms: i64 = match unit[0] {
        b's' => 1_000,
        b'm' => 60 * 1_000,
        b'h' => 60 * 60 * 1_000,
        b'd' => 24 * 60 * 60 * 1_000,
        b'w' => 7 * 24 * 60 * 60 * 1_000,
        _ => return None,
    };
    num.checked_mul(unit_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_epoch_ms_passthrough() {
        assert_eq!(
            parse_timestamp("1700000000000", 0).unwrap(),
            1_700_000_000_000
        );
    }

    #[test]
    fn parses_relative_offsets() {
        let now = 10_000_000;
        assert_eq!(parse_timestamp("5m", now).unwrap(), now - 5 * 60 * 1000);
        assert_eq!(parse_timestamp("2h", now).unwrap(), now - 2 * 3_600_000);
        assert_eq!(parse_timestamp("1d", now).unwrap(), now - 86_400_000);
        assert_eq!(parse_timestamp("1w", now).unwrap(), now - 7 * 86_400_000);
        assert_eq!(parse_timestamp("30s", now).unwrap(), now - 30_000);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_timestamp("five-minutes", 0).is_err());
        assert!(parse_timestamp("", 0).is_err());
        assert!(parse_timestamp("5x", 0).is_err());
    }
}
