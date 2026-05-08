use crate::cli::{SnapshotArgs, SnapshotOp};
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub async fn run(cwd: PathBuf, args: SnapshotArgs) -> Result<()> {
    let path = cwd.canonicalize().unwrap_or(cwd);
    match args.op {
        SnapshotOp::Create { label } => {
            let vault = open(&path).await?;
            vault.create_label(&label).await?;
            vault.flush().await?;
            println!("snapshot created: {}", label);
        }
        SnapshotOp::List => {
            let vault = open(&path).await?;
            let labels = vault.list_labels().await?;
            if labels.is_empty() {
                println!("(no snapshots)");
            } else {
                for l in labels {
                    println!("{} (created at {} ms)", l.name, l.created_at);
                }
            }
        }
        SnapshotOp::Restore { label } => {
            let cfg = require_config(&path)?;
            let mut vault = open(&path).await?;
            let _binding = vault
                .bind_directory(&path, cfg.sync.to_bind_options())
                .await?;
            vault.restore_label(&label).await?;
            vault.flush().await?;
            println!("restored to snapshot: {}", label);
        }
        SnapshotOp::Delete { label } => {
            let vault = open(&path).await?;
            vault.delete_label(&label).await?;
            vault.flush().await?;
            println!("snapshot deleted: {}", label);
        }
    }
    Ok(())
}

async fn open(path: &Path) -> Result<Vault> {
    let cfg = require_config(path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let identity = config::resolve_identity(path, &cfg)?;
    Ok(Vault::open(OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        identity,
        storage_path: path.join(".agentsync"),
        hub_pubkey: config::resolve_hub_pubkey(&cfg)?,
        name: cfg.vault.name.clone(),
    })
    .await?)
}
