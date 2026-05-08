use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;
use std::path::PathBuf;

pub async fn run(cwd: PathBuf) -> Result<()> {
    let path = cwd.canonicalize().unwrap_or(cwd);
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let identity = config::resolve_identity(&path, &cfg)?;
    let vault = Vault::open(OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        identity,
        storage_path: path.join(".agentsync"),
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
        name: cfg.vault.name.clone(),
    })
    .await?;
    // Re-saving the doc forces Automerge to repack columnar storage.
    vault.flush().await?;
    println!("compaction pass complete");
    Ok(())
}
