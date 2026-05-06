use crate::cli::CloneArgs;
use crate::config::{ConfigFile, KeySection, SyncSection, VaultSection};
use agentsync_core::{decode_key, OpenOptions, Vault};
use anyhow::{Context, Result};

pub async fn run(args: CloneArgs) -> Result<()> {
    let target = args.local_path.clone();
    if target.exists() && std::fs::read_dir(&target)?.next().is_some() {
        anyhow::bail!("clone target {} is not empty", target.display());
    }
    std::fs::create_dir_all(&target)?;
    let target = target.canonicalize()?;
    let storage = target.join(".agentsync");

    let key_b64 = args.key.clone().or_else(|| std::env::var("AGENTSYNC_KEY").ok())
        .context("--key not provided and AGENTSYNC_KEY env var not set")?;
    let key = decode_key(&key_b64)?;

    let cfg = ConfigFile {
        vault: VaultSection {
            id: Some(args.vault_id.clone()),
            rendezvous_url: args.rendezvous.clone(),
        },
        key: KeySection {
            source: Some("env".into()),
            keyring_name: None,
            key_b64: None,
            env_var: Some("AGENTSYNC_KEY".into()),
        },
        sync: SyncSection::default(),
    };
    crate::config::write(&target, &cfg)?;
    let bind_opts = cfg.sync.to_bind_options();

    let opts = OpenOptions {
        rendezvous_url: args.rendezvous.clone(),
        vault_id: args.vault_id.clone(),
        vault_key: key,
        storage_path: storage,
    };
    let mut vault = Vault::open(opts).await?;
    let _binding = vault.bind_directory(&target, bind_opts).await?;
    if args.rendezvous.is_some() {
        if let Err(e) = vault.connect().await {
            anyhow::bail!("connect to rendezvous failed: {}", e);
        }
        println!("connected; downloading vault state. Press Ctrl-C to stop after sync.");
        tokio::signal::ctrl_c().await?;
    } else {
        println!("offline clone; vault metadata written to {}", target.display());
    }
    vault.flush().await?;
    Ok(())
}
