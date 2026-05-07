use crate::cli::InitArgs;
use crate::config::{config_path, identity_path, write, ConfigFile, IdentitySection, SyncSection, VaultSection};
use agentsync_core::{CreateOptions, Identity, Vault};
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

    // Build the config first so we know where the identity should land.
    let mut cfg = ConfigFile {
        vault: VaultSection {
            id: None,
            rendezvous_url: args.rendezvous.clone(),
            hub_pubkey: None,
        },
        identity: IdentitySection {
            path: args
                .identity
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            agent_socket: None,
            agent_pubkey: None,
        },
        sync: SyncSection::default(),
    };

    // Reuse an existing identity at the configured path if there is one,
    // otherwise generate a fresh one.
    let id_path = identity_path(&path, &cfg);
    let identity = if id_path.exists() {
        Identity::load_from_file(&id_path).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let fresh = Identity::generate();
        fresh.save_to_file(&id_path).map_err(|e| anyhow::anyhow!(e))?;
        fresh
    };

    let opts = CreateOptions {
        rendezvous_url: args.rendezvous.clone(),
        identity: Some(identity.clone()),
        storage_path: storage,
    };
    let (vault, created) = Vault::create(opts).await?;
    cfg.vault.id = Some(created.vault_id.clone());
    write(&path, &cfg)?;
    vault.flush().await?;

    println!("Initialized agentsync vault.");
    println!("vault_id      = {}", created.vault_id);
    println!("identity_pub  = {}", identity.pubkey().to_ssh_string());
    println!("identity_path = {}", id_path.display());
    println!();
    println!(
        "Your pubkey is already authorized in peers.md. To authorize another \
         device, append its `agentsync key show` output as a new line in peers.md."
    );
    Ok(())
}
