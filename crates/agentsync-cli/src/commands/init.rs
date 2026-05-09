use crate::cli::InitArgs;
use crate::config::{
    ConfigFile, IdentitySection, SyncSection, VaultSection, config_path, identity_path, write,
};
use agentsync_core::{CreateOptions, Identity, Vault, normalize_rendezvous_url};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub async fn run(cwd: PathBuf, args: InitArgs) -> Result<()> {
    if !cwd.exists() {
        std::fs::create_dir_all(&cwd)
            .with_context(|| format!("create vault dir {}", cwd.display()))?;
    }
    let path = cwd.canonicalize().unwrap_or(cwd);
    let storage = path.join(".agentsync");
    if config_path(&path).exists() {
        anyhow::bail!(
            "vault already initialized at {} (config.toml exists)",
            path.display()
        );
    }

    let rendezvous = args.rendezvous.as_deref().map(normalize_rendezvous_url);
    let name = args.name.clone().or_else(|| default_name_from_path(&path));

    // Build the config first so we know where the identity should land.
    let mut cfg = ConfigFile {
        vault: VaultSection {
            id: None,
            name: name.clone(),
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
        fresh
            .save_to_file(&id_path)
            .map_err(|e| anyhow::anyhow!(e))?;
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
        seed_syncignore(&path.join(".syncignore"))?;
    }

    println!("Initialized agentsync vault.");
    println!("vault_id      = {}", created.vault_id);
    println!("name          = {}", name.as_deref().unwrap_or("(unnamed)"));
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

/// Default vault name derived from the basename of `path`. Returns `None`
/// for degenerate cases (root, empty, current-dir literals) so the caller
/// can decide whether to prompt or store an empty name.
fn default_name_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name)
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
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Default `.syncignore` body. Uses gitignore syntax — full negation,
/// directory-only patterns, and anchoring all work. `.agentsync/` and `.git/`
/// are excluded unconditionally by the engine and don't need to live here.
const DEFAULT_SYNCIGNORE: &str = "\
# .syncignore — patterns excluded from agentsync's sync engine.
# Same syntax as .gitignore. Add anything you don't want propagated to peers.

node_modules/
.DS_Store
";

/// Drop a starter `.syncignore` at `path` if none exists. Existing files are
/// left untouched — users may have customised them, and gitignore semantics
/// makes blind appending risky (a later `*` could shadow earlier negations).
fn seed_syncignore(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, DEFAULT_SYNCIGNORE)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
