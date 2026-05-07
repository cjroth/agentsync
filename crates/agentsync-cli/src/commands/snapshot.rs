use crate::cli::{SnapshotArgs, SnapshotOp};
use crate::commands::require_config;
use crate::config;
use agentsync_core::{OpenOptions, Vault};
use anyhow::Result;

pub async fn run(args: SnapshotArgs) -> Result<()> {
    match args.op {
        SnapshotOp::Create { label, path } => {
            let vault = open(&path).await?;
            vault.create_label(&label).await?;
            vault.flush().await?;
            println!("snapshot created: {}", label);
        }
        SnapshotOp::List { path } => {
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
        SnapshotOp::Restore { label, path } => {
            let canon = path.canonicalize().unwrap_or(path.clone());
            let cfg = require_config(&canon)?;
            let mut vault = open(&canon).await?;
            let _binding = vault
                .bind_directory(&canon, cfg.sync.to_bind_options())
                .await?;
            vault.restore_label(&label).await?;
            vault.flush().await?;
            println!("restored to snapshot: {}", label);
        }
        SnapshotOp::Delete { label, path } => {
            let vault = open(&path).await?;
            vault.delete_label(&label).await?;
            vault.flush().await?;
            println!("snapshot deleted: {}", label);
        }
    }
    Ok(())
}

async fn open(path: &std::path::Path) -> Result<Vault> {
    let path = path.canonicalize().unwrap_or(path.to_path_buf());
    let cfg = require_config(&path)?;
    let vault_id = cfg.vault.id.clone().unwrap();
    let key = config::resolve_key(&cfg, None)?;
    Ok(Vault::open(OpenOptions {
        rendezvous_url: cfg.vault.rendezvous_url.clone(),
        vault_id,
        vault_key: key,
        storage_path: path.join(".agentsync"),
    })
    .await?)
}
