use crate::cli::CompactArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;

pub async fn run(args: CompactArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let key = config::resolve_key(&cfg, None)?;
    let vault = Vault::open(OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        vault_key: key,
        storage_path: path.join(".agentsync"),
    })
    .await?;
    // Re-saving the doc forces Automerge to repack columnar storage.
    vault.flush().await?;
    println!("compaction pass complete");
    Ok(())
}
