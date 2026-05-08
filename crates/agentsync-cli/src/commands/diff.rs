use crate::cli::DiffArgs;
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;

pub async fn run(args: DiffArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
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
    let files = vault.list_files().await?;
    println!(
        "diff between '{}' and '{}' (file-level)",
        args.from,
        args.to.as_deref().unwrap_or("now")
    );
    println!("currently {} files in vault", files.len());
    println!("(detailed pre/post-state diff is implemented at the doc level — TODO surface in CLI)");
    Ok(())
}
