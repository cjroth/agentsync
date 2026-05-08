use crate::cli::{KeyArgs, KeyOp};
use crate::config::{identity_path, read_or_default, write, ConfigFile, IdentitySection};
use agentsync_core::Identity;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub async fn run(cwd: PathBuf, args: KeyArgs) -> Result<()> {
    let path = cwd.canonicalize().unwrap_or(cwd);
    match args.op {
        KeyOp::Generate { identity } => {
            let mut cfg = read_or_default(&path)?;
            if let Some(p) = identity {
                cfg.identity.path = Some(p.to_string_lossy().into_owned());
            }
            let id_path = identity_path(&path, &cfg);
            if id_path.exists() {
                anyhow::bail!(
                    "identity file already exists at {}; refusing to overwrite",
                    id_path.display()
                );
            }
            let id = Identity::generate();
            id.save_to_file(&id_path).map_err(|e| anyhow::anyhow!(e))?;
            // Persist the identity-path config if a non-default was passed.
            if cfg.identity.path.is_some() {
                if cfg.identity.agent_socket.is_none() && cfg.identity.agent_pubkey.is_none() {
                    cfg.identity = IdentitySection {
                        path: cfg.identity.path,
                        ..Default::default()
                    };
                }
                let _ = write(&path, &cfg);
            }
            println!("{}", id.pubkey().to_ssh_string());
            eprintln!("identity written to {}", id_path.display());
        }
        KeyOp::Show => {
            let cfg = read_or_default(&path).unwrap_or_else(|_| ConfigFile::default());
            // Agent-backed identity: print the configured agent_pubkey.
            if let Some(s) = cfg.identity.agent_pubkey.as_deref() {
                println!("{}", s.trim());
            } else {
                let id_path = identity_path(&path, &cfg);
                let id = Identity::load_from_file(&id_path)
                    .with_context(|| format!("load identity from {}", id_path.display()))?;
                println!("{}", id.pubkey().to_ssh_string());
            }
        }
    }
    Ok(())
}
