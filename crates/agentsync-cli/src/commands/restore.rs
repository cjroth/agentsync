use crate::cli::RestoreAtArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;

pub async fn run_restore_at(args: RestoreAtArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let identity = config::resolve_identity(&path, &cfg)?;

    let target_ms = parse_timestamp(&args.timestamp)?;
    let opts = OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        identity,
        storage_path: path.join(".agentsync"),
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
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

fn parse_timestamp(s: &str) -> Result<i64> {
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    // Very lightweight RFC3339 parser using chrono-free approach is awkward;
    // for v1, accept either epoch ms or a small set of forms.
    if let Some((year, rest)) = s.split_once('-') {
        // YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS
        let _ = year;
        let _ = rest;
        anyhow::bail!(
            "RFC3339 timestamp parsing not yet supported in this build; pass epoch milliseconds"
        );
    }
    anyhow::bail!("unrecognised timestamp: {}", s)
}
