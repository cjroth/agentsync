use crate::cli::CloneArgs;
use crate::config::{ConfigFile, KeySection, SyncSection, VaultSection};
use agentsync_core::{decode_key, discover_vault_id, OpenOptions, Vault};
use anyhow::{Context, Result};

pub async fn run(args: CloneArgs) -> Result<()> {
    let target = args.local_path.clone();
    if target.exists() && std::fs::read_dir(&target)?.next().is_some() {
        anyhow::bail!("clone target {} is not empty", target.display());
    }
    std::fs::create_dir_all(&target)?;
    let target = target.canonicalize()?;
    let storage = target.join(".agentsync");

    let key_b64 = args
        .key
        .clone()
        .or_else(|| std::env::var("AGENTSYNC_KEY").ok())
        .context("--key not provided and AGENTSYNC_KEY env var not set")?;
    let key = decode_key(&key_b64)?;

    // Discover the vault_id from the server if the user didn't pass one.
    // This is the option-1 UX: the rendezvous URL + key is all the user needs.
    let vault_id = match args.vault_id.clone() {
        Some(v) => v,
        None => {
            println!("discovering vault at {}…", args.rendezvous);
            discover_vault_id(&args.rendezvous, key)
                .await
                .with_context(|| format!("discover vault_id at {}", args.rendezvous))?
        }
    };
    println!("vault_id: {}", vault_id);

    let cfg = ConfigFile {
        vault: VaultSection {
            id: Some(vault_id.clone()),
            rendezvous_url: Some(args.rendezvous.clone()),
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
        rendezvous_url: Some(args.rendezvous.clone()),
        vault_id,
        vault_key: key,
        storage_path: storage,
    };
    let mut vault = Vault::open(opts).await?;
    let _binding = vault.bind_directory(&target, bind_opts).await?;
    if let Err(e) = vault.connect().await {
        anyhow::bail!("connect to rendezvous failed: {}", e);
    }
    println!("connected to {}", args.rendezvous);
    println!("syncing into {} — Ctrl-C to stop.", target.display());
    tokio::signal::ctrl_c().await?;
    vault.flush().await?;
    Ok(())
}
