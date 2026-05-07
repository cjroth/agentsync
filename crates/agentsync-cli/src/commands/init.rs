use crate::cli::InitArgs;
use crate::config::{config_path, identity_path, write, ConfigFile, IdentitySection, SyncSection, VaultSection};
use agentsync_core::{normalize_rendezvous_url, CreateOptions, Identity, Vault};
use anyhow::{Context, Result};
use std::path::Path;

pub async fn run(args: InitArgs) -> Result<()> {
    let path = args.path.canonicalize().unwrap_or(args.path.clone());
    let storage = path.join(".agentsync");
    if config_path(&path).exists() {
        anyhow::bail!(
            "vault already initialized at {} (config.toml exists)",
            path.display()
        );
    }

    let rendezvous = args.rendezvous.as_deref().map(normalize_rendezvous_url);

    // Build the config first so we know where the identity should land.
    let mut cfg = ConfigFile {
        vault: VaultSection {
            id: None,
            rendezvous_url: rendezvous.clone(),
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
        rendezvous_url: rendezvous.clone(),
        identity: Some(identity.clone()),
        storage_path: storage,
    };
    let (vault, created) = Vault::create(opts).await?;
    cfg.vault.id = Some(created.vault_id.clone());
    write(&path, &cfg)?;
    vault.flush().await?;

    if !args.no_ignore_files {
        ensure_ignore_entry(&path.join(".gitignore"), ".agentsync/")?;
        ensure_ignore_entry(&path.join(".agentsignore"), ".agentsync/")?;
    }

    println!("Initialized agentsync vault.");
    println!("vault_id      = {}", created.vault_id);
    println!("identity_pub  = {}", identity.pubkey().to_ssh_string());
    println!("identity_path = {}", id_path.display());
    println!();
    println!(
        "Your pubkey is already authorized in authorized_keys. To authorize \
         another device, append its `agentsync key show` output as a new line \
         in authorized_keys."
    );
    Ok(())
}

/// Ensure `path` contains `entry` on its own line. Creates the file if
/// missing; appends if the entry isn't already present (line-exact match
/// after trimming).
fn ensure_ignore_entry(path: &Path, entry: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).ok();
    let already = existing
        .as_deref()
        .map(|s| s.lines().any(|l| l.trim() == entry))
        .unwrap_or(false);
    if already {
        return Ok(());
    }
    let mut body = existing.unwrap_or_default();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(entry);
    body.push('\n');
    std::fs::write(path, body)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
