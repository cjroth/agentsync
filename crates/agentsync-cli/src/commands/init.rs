use crate::cli::InitArgs;
use crate::config::{config_path, write, ConfigFile, KeySection, SyncSection, VaultSection};
use agentsync_core::{encode_key, CreateOptions, Vault};
use anyhow::Result;

pub async fn run(args: InitArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let storage = path.join(".agentsync");
    if config_path(&path).exists() {
        anyhow::bail!(
            "vault already initialized at {} (config.toml exists)",
            path.display()
        );
    }
    let opts = CreateOptions {
        rendezvous_url: args.rendezvous.clone(),
        vault_key: None,
        storage_path: storage,
    };
    let (vault, created) = Vault::create(opts).await?;
    let cfg = ConfigFile {
        vault: VaultSection {
            id: Some(created.vault_id.clone()),
            rendezvous_url: args.rendezvous.clone(),
        },
        key: KeySection {
            source: Some(args.key_source.clone().unwrap_or_else(|| "env".to_string())),
            keyring_name: None,
            key_b64: None,
            env_var: Some("AGENTSYNC_KEY".to_string()),
        },
        sync: SyncSection::default(),
    };
    write(&path, &cfg)?;
    vault.flush().await?;

    println!("Initialized agentsync vault.");
    println!("vault_id   = {}", created.vault_id);
    println!("vault_key  = {}", encode_key(&created.vault_key));
    println!();
    println!("Set the key in your environment to start syncing:");
    println!("  export AGENTSYNC_KEY={}", encode_key(&created.vault_key));
    Ok(())
}
