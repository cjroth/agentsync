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
    let opts = OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id: vault_id.clone(),
        identity,
        storage_path: path.join(".agentsync"),
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
        name: cfg.vault.name.clone(),
    };
    let vault = Vault::open(opts).await?;
    let files = vault.list_files().await?;
    println!("vault_id:       {}", vault_id);
    println!(
        "name:           {}",
        cfg.vault.name.as_deref().unwrap_or("(unnamed)")
    );
    println!("storage:        {}", vault.storage_path().display());
    println!(
        "rendezvous:     {}",
        cfg.vault.rendezvous_url.as_deref().unwrap_or("(none)")
    );
    println!("identity_pub:   {}", vault.pubkey().to_ssh_string());
    println!("file count:     {}", files.len());
    Ok(())
}
