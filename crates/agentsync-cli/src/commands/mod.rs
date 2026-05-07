pub mod init;
pub mod watch;
pub mod clone;
pub mod status;
pub mod push_pull;
pub mod restore;
pub mod snapshot;
pub mod diff;
pub mod compact;
pub mod key;
pub mod hub;

use std::path::Path;

pub fn require_config(path: &Path) -> anyhow::Result<crate::config::ConfigFile> {
    let cfg = crate::config::read_or_default(path)?;
    if cfg.vault.id.is_none() {
        anyhow::bail!(
            "no vault configured at {} — run `agentsync init` or `agentsync clone` first",
            path.display()
        );
    }
    Ok(cfg)
}
