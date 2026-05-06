use crate::cli::StatusArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;

pub async fn run(args: StatusArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let key = config::resolve_key(&cfg, None)?;
    let opts = OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id: vault_id.clone(),
        vault_key: key,
        storage_path: path.join(".agentsync"),
    };
    let vault = Vault::open(opts).await?;
    let files = vault.list_files().await?;
    println!("vault_id:       {}", vault_id);
    println!("storage:        {}", vault.storage_path().display());
    println!(
        "rendezvous:     {}",
        cfg.vault.rendezvous_url.as_deref().unwrap_or("(none)")
    );
    println!("file count:     {}", files.len());
    Ok(())
}
