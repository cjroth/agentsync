use crate::cli::{KeyArgs, KeyOp};
use crate::commands::require_config;
use crate::config;
use agentsync_core::{encode_key, generate_vault_key};
use anyhow::Result;

pub async fn run(args: KeyArgs) -> Result<()> {
    match args.op {
        KeyOp::Generate => {
            let k = generate_vault_key();
            println!("{}", encode_key(&k));
        }
        KeyOp::Show { path } => {
            let path = path.canonicalize().unwrap_or(path);
            let cfg = require_config(&path)?;
            let key = config::resolve_key(&cfg, None)?;
            println!("{}", encode_key(&key));
        }
        KeyOp::Store { path } => {
            let path = path.canonicalize().unwrap_or(path);
            let cfg = require_config(&path)?;
            let key = config::resolve_key(&cfg, None)?;
            // Persist into config.toml as an inline file source. (Keyring
            // backend is out of scope for v1.)
            let mut new_cfg = cfg.clone();
            new_cfg.key.source = Some("file".into());
            new_cfg.key.key_b64 = Some(encode_key(&key));
            config::write(&path, &new_cfg)?;
            println!("key stored in {}", config::config_path(&path).display());
        }
    }
    Ok(())
}
