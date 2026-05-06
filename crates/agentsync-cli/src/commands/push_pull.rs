use crate::cli::PushPullArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;
use std::time::Duration;

pub async fn run_push(args: PushPullArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let key = config::resolve_key(&cfg, None)?;
    let opts = OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        vault_key: key,
        storage_path: path.join(".agentsync"),
    };
    let mut vault = Vault::open(opts).await?;
    let _binding = vault
        .bind_directory(&path, cfg.sync.to_bind_options())
        .await?;
    if cfg.vault.rendezvous_url.is_some() {
        if let Err(e) = vault.connect().await {
            anyhow::bail!("connect: {}", e);
        }
        // Give the sync hub a moment to drain.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    vault.flush().await?;
    println!("push complete");
    Ok(())
}

pub async fn run_pull(args: PushPullArgs) -> Result<()> {
    // For Automerge sync, push and pull are symmetric — connecting briefly
    // exchanges all changes in both directions.
    run_push(args).await?;
    println!("pull complete");
    Ok(())
}
